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
pub const SLOT_COUNT: u32 = 8;

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
}

impl Default for Settings {
    fn default() -> Self {
        Self { codec: "H264".into(), max_bitrate_kbps: 12_000, max_fps: 60 }
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
    pub slots: BTreeMap<u32, SlotAssign>,
    pub tracks: Value,
    pub settings: Settings,
    pub publisher_ready: bool,
    pub thumbs: bool,
    pub groups: Groups,
    pub obs: ObsConfig,
    config_path: Option<PathBuf>,
}

impl Hub {
    pub fn new(port: u16) -> Self {
        Self {
            app: None,
            port,
            viewers: HashMap::new(),
            next_id: 1,
            slots: (1..=SLOT_COUNT).map(|i| (i, SlotAssign::default())).collect(),
            tracks: json!([]),
            settings: Settings::default(),
            publisher_ready: false,
            thumbs: true,
            groups: Groups::default(),
            obs: ObsConfig::default(),
            config_path: None,
        }
    }

    // ---------- persistenza ----------
    pub fn load(&mut self, path: PathBuf) {
        if let Ok(text) = std::fs::read_to_string(&path) {
            match serde_json::from_str::<Persisted>(&text) {
                Ok(p) => {
                    self.settings = p.settings;
                    self.groups = Groups::from_configs(p.groups);
                    self.obs = p.obs;
                }
                Err(e) => eprintln!("[obsyard] config non valida, ignorata: {e}"),
            }
        }
        self.config_path = Some(path);
    }

    fn persist(&self) {
        let Some(path) = &self.config_path else { return };
        let p = Persisted { settings: self.settings.clone(), groups: self.groups.configs.clone(), obs: self.obs.clone() };
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

    /// Ricalcola gli slot guidati dai gruppi. Ritorna true se qualche slot è cambiato.
    fn apply_groups(&mut self) -> bool {
        let live = self.live_video();
        self.groups.rebind(&live);
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
        let id = self.groups.save(g, SLOT_COUNT)?;
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
            "slots": self.slots,
            "viewers": counts,
            "settings": self.settings,
            "groups": groups,
            "obs": self.obs,
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
            "thumbs": self.thumbs,
        }));
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

    pub fn set_settings(&mut self, s: Settings) {
        self.settings = s;
        self.persist();
        self.to_studio(json!({"type": "settings", "settings": self.settings}));
        self.emit_state();
    }

    pub fn set_thumbs(&mut self, on: bool) {
        self.thumbs = on;
        self.to_studio(json!({"type": "thumbs", "on": on}));
    }
}
