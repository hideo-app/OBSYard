//! Stato condiviso + routing dei messaggi di signaling.
//! Nessun frame video passa di qui: solo SDP/ICE/comandi (pochi KB).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc::UnboundedSender;

use crate::groups::{GroupConfig, Groups};
use std::path::PathBuf;

pub type SharedHub = Arc<Mutex<Hub>>;

/// Uscita OBS (Fonte browser): id stabile (URL /slot/{id}), nome, ordine = posizione nel vettore.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct Output {
    pub id: u32,
    pub name: String,
    /// eccezione allo sfondo globale senza video: "transparent" o "black" (null = come l'impostazione globale)
    #[serde(default)]
    pub bg: Option<String>,
}

fn default_outputs() -> Vec<Output> {
    (1..=4).map(|i| Output { id: i, name: format!("Cam {i}"), bg: None }).collect()
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct SlotAssign {
    pub video: Option<String>,
    /// id traccia, "auto" (audio dello stesso ospite) o null
    pub audio: Option<String>,
}

impl Default for SlotAssign {
    fn default() -> Self {
        Self { video: None, audio: Some("auto".into()) }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// "H264" (encoder hardware su Windows), "VP8", "VP9", "AV1"
    pub codec: String,
    pub max_bitrate_kbps: u32,
    pub max_fps: u32,
    /// nome del layout personalizzato di StreamYard con tutti i riquadri a piena altezza
    #[serde(default = "default_full_res_layout")]
    pub full_res_layout: String,
    /// chi va in onda entra da solo nel primo gruppo, in ordine di arrivo
    #[serde(default)]
    pub auto_add: bool,
    /// piattaforma aperta nella finestra studio: "streamyard" o "discord"
    #[serde(default = "default_platform")]
    pub platform: String,
    /// cosa mostrano le uscite senza video (vuote, fuori onda, camera spenta): "transparent" o "black"
    #[serde(default = "default_empty_bg")]
    pub empty_bg: String,
    /// nomi con precedenza, in ordine (entrano davanti agli altri)
    #[serde(default)]
    pub priority: Vec<String>,
}

fn default_platform() -> String {
    "streamyard".into()
}

/// pagina da aprire nella finestra studio per la piattaforma scelta
pub fn platform_url(platform: &str) -> &'static str {
    match platform {
        "discord" => "https://discord.com/app",
        _ => "https://streamyard.com/",
    }
}

fn default_empty_bg() -> String {
    "transparent".into()
}

fn default_full_res_layout() -> String {
    "OBSYard".into()
}

impl Default for Settings {
    fn default() -> Self {
        Self { codec: "H264".into(), max_bitrate_kbps: 12_000, max_fps: 60, full_res_layout: default_full_res_layout(), auto_add: false, platform: default_platform(), empty_bg: default_empty_bg(), priority: vec![] }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ObsConfig {
    /// indirizzo obs-websocket (OBS → Strumenti → Impostazioni server WebSocket)
    pub url: String,
    #[serde(default)]
    pub password: String,
    /// "Pilota OBS": i gruppi eseguono le azioni per numero di partecipanti
    #[serde(default)]
    pub drive: bool,
}

impl Default for ObsConfig {
    fn default() -> Self {
        Self { url: "ws://127.0.0.1:4455".into(), password: String::new(), drive: false }
    }
}

/// Configurazione salvata su disco (config.json nella cartella dati dell'app).
#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct Persisted {
    settings: Settings,
    outputs: Vec<Output>,
    groups: Vec<GroupConfig>,
    obs: ObsConfig,
}

pub struct Viewer {
    pub slot: u32,
    pub tx: UnboundedSender<String>,
}

pub struct Hub {
    pub app: Option<AppHandle>,
    pub port: u16,
    pub viewers: HashMap<u64, Viewer>,
    next_id: u64,
    pub outputs: Vec<Output>,
    pub slots: BTreeMap<u32, SlotAssign>,
    pub tracks: Value,
    pub settings: Settings,
    pub publisher_ready: bool,
    pub thumbs: bool,
    pub groups: Groups,
    pub obs: ObsConfig,
    config_path: Option<PathBuf>,
    /// toggle "ospiti a piena risoluzione" e stato riportato dallo studio
    pub full_res: bool,
    pub full_res_status: String,
}

impl Hub {
    pub fn new(port: u16) -> Self {
        Self {
            app: None,
            port,
            viewers: HashMap::new(),
            next_id: 1,
            outputs: default_outputs(),
            slots: (1..=4).map(|i| (i, SlotAssign::default())).collect(),
            tracks: json!([]),
            settings: Settings::default(),
            publisher_ready: false,
            thumbs: true,
            groups: Groups::default(),
            obs: ObsConfig::default(),
            config_path: None,
            full_res: false,
            full_res_status: "off".into(),
        }
    }

    // ---------- persistenza ----------
    pub fn load(&mut self, path: PathBuf) {
        if let Ok(text) = std::fs::read_to_string(&path) {
            match serde_json::from_str::<Persisted>(&text) {
                Ok(p) => {
                    self.settings = p.settings;
                    if !p.outputs.is_empty() {
                        self.outputs = p.outputs;
                    }
                    self.groups = Groups::from_configs(p.groups);
                    self.obs = p.obs;
                }
                Err(e) => eprintln!("[obsyard] config non valida, ignorata: {e}"),
            }
        }
        self.sync_slots();
        self.config_path = Some(path);
    }

    /// una SlotAssign per ogni uscita, nessuna in più
    fn sync_slots(&mut self) {
        let ids: Vec<u32> = self.outputs.iter().map(|o| o.id).collect();
        self.slots.retain(|k, _| ids.contains(k));
        for id in ids {
            self.slots.entry(id).or_default();
        }
    }

    pub fn has_output(&self, id: u32) -> bool {
        self.outputs.iter().any(|o| o.id == id)
    }

    fn push_outputs(&self) {
        self.to_studio(json!({"type": "outputs", "outputs": self.outputs}));
    }

    // ---------- uscite (dinamiche, come guestroom) ----------
    pub fn output_add(&mut self, name: Option<String>) -> u32 {
        let id = self.outputs.iter().map(|o| o.id).max().unwrap_or(0) + 1;
        // prossimo "Cam N"
        let n = self
            .outputs
            .iter()
            .filter_map(|o| o.name.strip_prefix("Cam ").and_then(|n| n.trim().parse::<u32>().ok()))
            .max()
            .unwrap_or(0)
            + 1;
        let name = name.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).unwrap_or(format!("Cam {n}"));
        self.outputs.push(Output { id, name, bg: None });
        self.sync_slots();
        self.persist();
        self.push_outputs();
        self.push_slots();
        self.emit_state();
        id
    }

    /// bg: None = non cambiare, "default" = come l'impostazione globale, "transparent" / "black"
    pub fn output_update(&mut self, id: u32, name: Option<String>, bg: Option<String>) -> Result<(), String> {
        let o = self.outputs.iter_mut().find(|o| o.id == id).ok_or("uscita inesistente")?;
        if let Some(n) = name {
            let n = n.trim();
            if n.is_empty() {
                return Err("il nome non può essere vuoto".into());
            }
            o.name = n.chars().take(40).collect();
        }
        if let Some(b) = bg {
            o.bg = match b.as_str() {
                "transparent" | "black" => Some(b),
                _ => None,
            };
        }
        self.persist();
        self.push_outputs();
        self.emit_state();
        Ok(())
    }

    pub fn output_move(&mut self, id: u32, delta: i32) {
        let Some(i) = self.outputs.iter().position(|o| o.id == id) else { return };
        let j = i as i32 + delta;
        if j < 0 || j >= self.outputs.len() as i32 {
            return;
        }
        self.outputs.swap(i, j as usize);
        self.persist();
        self.emit_state();
    }

    pub fn output_delete(&mut self, id: u32) {
        if !self.has_output(id) {
            return;
        }
        self.outputs.retain(|o| o.id != id);
        self.groups.remove_slot(id); // esce dal gruppo; un gruppo rimasto vuoto sparisce
        self.sync_slots();
        self.persist();
        self.apply_groups();
        self.push_outputs();
        self.push_slots();
        self.emit_state();
    }

    fn persist(&self) {
        let Some(path) = &self.config_path else { return };
let p = Persisted {
            settings: self.settings.clone(),
            outputs: self.outputs.clone(),
            groups: self.groups.configs.clone(),
            obs: self.obs.clone(),
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string_pretty(&p) {
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, path);
            }
        }
    }

    // ---------- gruppi ----------
    /// tracce video connesse: (id, nome)
    fn live_video(&self) -> Vec<(String, String)> {
        self.tracks
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|t| t["kind"] == "video")
                    .filter_map(|t| Some((t["id"].as_str()?.to_string(), t["label"].as_str().unwrap_or("").to_string())))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// webcam in onda (niente schermi condivisi), nell'ordine in cui sono arrivate
    fn live_webcams(&self) -> Vec<(String, String)> {
        self.tracks
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|t| t["kind"] == "video" && t["origin"] != "screen")
                    .filter_map(|t| Some((t["id"].as_str()?.to_string(), t["label"].as_str().unwrap_or("").to_string())))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// il primo gruppo nell'ordine delle uscite (quello in cima nella Regia)
    fn first_group(&self) -> Option<u32> {
        self.outputs.iter().find_map(|o| self.groups.group_of_slot(o.id))
    }

    /// Ricalcola gli slot guidati dai gruppi. Ritorna true se qualche slot è cambiato.
    fn apply_groups(&mut self) -> bool {
        let live = self.live_video();
        let priority = self.settings.priority.clone();
        self.groups.rebind(&live, &priority);
        if self.settings.auto_add {
            if let Some(gid) = self.first_group() {
                let webcams = self.live_webcams();
                self.groups.auto_add(gid, &webcams, &priority);
            }
        }
        let is_live = |t: &str| live.iter().any(|(id, _)| id == t);
        let mut changed = false;
        for (slot, track) in self.groups.fill(&is_live) {
            let a = SlotAssign { video: track, audio: Some("auto".into()) };
            let cur = self.slots.get(&slot);
            if cur.map(|c| c.video != a.video || c.audio != a.audio).unwrap_or(true) {
                self.slots.insert(slot, a);
                changed = true;
            }
        }
        changed
    }

    fn push_slots(&self) {
        self.to_studio(json!({"type": "slots", "slots": self.slots}));
    }

    pub fn set_tracks(&mut self, tracks: Value) {
        self.tracks = tracks;
        if self.apply_groups() {
            self.push_slots();
        }
        self.emit_state();
    }

    pub fn group_save(&mut self, g: GroupConfig) -> Result<u32, String> {
        let valid: Vec<u32> = self.outputs.iter().map(|o| o.id).collect();
        let id = self.groups.save(g, &valid)?;
        self.persist();
        if self.apply_groups() {
            self.push_slots();
        }
        self.emit_state();
        Ok(id)
    }

    pub fn group_delete(&mut self, id: u32) {
        // gli slot del gruppo restano con quello che mostrano
        self.groups.delete(id);
        self.persist();
        self.emit_state();
    }

    pub fn group_remove(&mut self, id: u32, track: &str) {
        self.groups.remove(id, track);
        if self.apply_groups() {
            self.push_slots();
        }
        self.emit_state();
    }

    fn push_full_res(&self) {
        self.to_studio(json!({"type": "fullRes", "on": self.full_res, "layout": self.settings.full_res_layout}));
    }

    pub fn set_full_res(&mut self, on: bool) {
        self.full_res = on;
        self.push_full_res();
        self.emit_state();
    }

    pub fn full_res_report(&mut self, status: String) {
        self.full_res_status = status;
        self.emit_state();
    }

    pub fn set_obs(&mut self, obs: ObsConfig) {
        self.obs = obs;
        self.persist();
        self.emit_state();
    }

    // ---------- verso la pagina StreamYard ----------
    fn to_studio(&self, msg: Value) {
        if let Some(w) = self.app.as_ref().and_then(|a| a.get_webview_window("studio")) {
            let _ = w.eval(format!("window.__obsyard&&window.__obsyard.recv({msg})"));
        }
    }

    // ---------- verso la UI di controllo ----------
    pub fn snapshot(&self) -> Value {
        let mut counts: BTreeMap<u32, u32> = BTreeMap::new();
        for v in self.viewers.values() {
            *counts.entry(v.slot).or_default() += 1;
        }
        let live = self.live_video();
        let groups = self.groups.view(&|t: &str| live.iter().any(|(id, _)| id == t));
        json!({
            "port": self.port,
            "publisherReady": self.publisher_ready,
            "tracks": self.tracks,
            "outputs": self.outputs,
            "slots": self.slots,
            "viewers": counts,
            "settings": self.settings,
            "groups": groups,
            "obs": self.obs,
            "fullRes": { "on": self.full_res, "status": self.full_res_status, "layout": self.settings.full_res_layout },
        })
    }

    pub fn emit_state(&self) {
        if let Some(app) = &self.app {
            let _ = app.emit_to("control", "state", self.snapshot());
        }
    }

    pub fn emit_thumbs(&self, thumbs: Value) {
        if let Some(app) = &self.app {
            let _ = app.emit_to("control", "thumbs", thumbs);
        }
    }

    // ---------- verso i viewer OBS ----------
    pub fn to_viewer(&self, id: u64, msg: &Value) {
        if let Some(v) = self.viewers.get(&id) {
            let _ = v.tx.send(msg.to_string());
        }
    }

    // ---------- eventi ----------
    pub fn publisher_ready(&mut self) {
        self.publisher_ready = true;
        self.tracks = json!([]);
        self.to_studio(json!({
            "type": "init",
            "settings": self.settings,
            "slots": self.slots,
            "outputs": self.outputs,
            "thumbs": self.thumbs,
        }));
        self.push_full_res();
        // La pagina è stata (ri)caricata: le vecchie PeerConnection sono morte.
        for (id, v) in &self.viewers {
            let _ = v.tx.send(json!({"type": "reset"}).to_string());
            self.to_studio(json!({"type": "viewer-join", "viewerId": id, "slot": v.slot}));
        }
        self.emit_state();
    }

    pub fn viewer_join(&mut self, slot: u32, tx: UnboundedSender<String>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.viewers.insert(id, Viewer { slot, tx });
        if self.publisher_ready {
            self.to_studio(json!({"type": "viewer-join", "viewerId": id, "slot": slot}));
        }
        self.emit_state();
        id
    }

    pub fn viewer_signal(&self, id: u64, msg: Value) {
        self.to_studio(json!({"type": "signal", "viewerId": id, "msg": msg}));
    }

    pub fn viewer_leave(&mut self, id: u64) {
        if self.viewers.remove(&id).is_some() {
            self.to_studio(json!({"type": "viewer-leave", "viewerId": id}));
            self.emit_state();
        }
    }

    /// Cerca una traccia per id esatto o per nome (case-insensitive, sottostringa).
    pub fn find_track(&self, kind: &str, q: &str) -> Option<String> {
        let list = self.tracks.as_array()?;
        let of_kind = || list.iter().filter(|t| t["kind"] == kind);
        if let Some(t) = of_kind().find(|t| t["id"] == q) {
            return t["id"].as_str().map(String::from);
        }
        let q = q.to_lowercase();
        of_kind()
            .find(|t| t["label"].as_str().is_some_and(|l| l.to_lowercase().contains(&q)))
            .and_then(|t| t["id"].as_str().map(String::from))
    }

    /// Assegna uno slot. Se lo slot è di un gruppo, passa dal gruppo:
    /// mandarci qualcuno = scambio/inserimento, svuotarlo = toglierlo dal gruppo.
    pub fn assign(&mut self, slot: u32, a: SlotAssign) {
        if let Some(gid) = self.groups.group_of_slot(slot) {
            let live = self.live_video();
            let is_live = |t: &str| live.iter().any(|(id, _)| id == t);
            match &a.video {
                Some(track) => {
                    let label = live.iter().find(|(id, _)| id == track).map(|(_, l)| l.clone()).unwrap_or_default();
                    self.groups.place(gid, slot, track, &label, &is_live);
                }
                None => self.groups.clear_slot(gid, slot, &is_live),
            }
            self.apply_groups();
        } else {
            self.slots.insert(slot, a);
        }
        self.push_slots();
        self.emit_state();
    }

    pub fn set_settings(&mut self, mut s: Settings) {
        if s.platform != "discord" {
            s.platform = "streamyard".into();
        }
        if s.platform != self.settings.platform {
            // cambio piattaforma: la finestra studio apre l'altro sito; tracce e uscite si svuotano da sole
            if let Some(w) = self.app.as_ref().and_then(|a| a.get_webview_window("studio")) {
                if let Ok(u) = platform_url(&s.platform).parse() {
                    let _ = w.navigate(u);
                }
            }
            self.full_res = false;
        }
        s.priority = s.priority.iter().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect();
        let layout_changed = s.full_res_layout != self.settings.full_res_layout;
        let priority_changed = s.priority != self.settings.priority;
        self.settings = s;
        if layout_changed {
            self.push_full_res();
        }
        if priority_changed {
            // elenco cambiato: i membri dei gruppi si riordinano una volta
            self.groups.sort_by_priority(&self.settings.priority);
        }
        // aggiunta automatica appena attivata / precedenza cambiata: ricalcola le uscite
        if self.apply_groups() {
            self.push_slots();
        }
        self.persist();
        self.to_studio(json!({"type": "settings", "settings": self.settings}));
        self.emit_state();
    }

    pub fn set_thumbs(&mut self, on: bool) {
        self.thumbs = on;
        self.to_studio(json!({"type": "thumbs", "on": on}));
    }
}
