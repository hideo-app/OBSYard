//! Gruppi di output (come i "camera groups" di guestroom.live).
//!
//! Un gruppo = elenco ordinato di slot (ordine di riempimento) + elenco ordinato di membri.
//! Il membro connesso n. 1 va sul primo slot, il 2 sul secondo, ecc.; gli slot avanzati restano vuoti.
//! - Contano solo i membri connessi: se uno esce gli altri salgono subito, anche in onda.
//! - Chi si riconnette con lo stesso nome riprende il suo posto (rebind per nome).
//! - Mandare qualcuno su uno slot del gruppo: un membro si SCAMBIA con chi è lì,
//!   un nuovo arrivato viene INSERITO lì e chi finisce oltre l'ultimo slot va in standby.
//! - Svuotare uno slot del gruppo toglie dal gruppo chi c'era; gli altri salgono.
//! - Aggiunta automatica (opzione globale): chi va in onda e non è in nessun gruppo entra nel
//!   PRIMO gruppo, in ordine di arrivo. Chi è stato tolto a mano non viene riaggiunto.
//! - Precedenza (elenco globale di nomi): chi è nell'elenco entra davanti a chi ha meno
//!   precedenza (o non è nell'elenco). Gli spostamenti manuali non vengono annullati.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Clone, Serialize, Deserialize, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GroupAction {
    /// scena di programma da attivare per questo numero di partecipanti
    #[serde(default)]
    pub scene: Option<String>,
    /// elementi da accendere: "Scena › Fonte" o "Gruppo OBS › Fonte"
    #[serde(default)]
    pub items: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GroupConfig {
    #[serde(default)]
    pub id: u32,
    pub name: String,
    /// slot in ordine di riempimento
    pub slots: Vec<u32>,
    /// azioni OBS per numero di partecipanti (0..=slots.len())
    #[serde(default)]
    pub actions: BTreeMap<u32, GroupAction>,
    /// altre scene su cui il gruppo può agire oltre a quelle nominate nelle azioni
    #[serde(default)]
    pub extra_scenes: Vec<String>,
    /// altri set di scene (es. "Reaction"): la scena di programma di OBS dice quale agisce
    #[serde(default)]
    pub sets: Vec<GroupSceneSet>,
}

/// Set di scene oltre a quello principale del gruppo (come gli "scene sets" di guestroom.live):
/// ha le sue azioni per numero di partecipanti e le sue scene extra.
#[derive(Clone, Serialize, Deserialize, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GroupSceneSet {
    pub name: String,
    #[serde(default)]
    pub actions: BTreeMap<u32, GroupAction>,
    #[serde(default)]
    pub extra_scenes: Vec<String>,
}

/// set di scene oltre a quello principale, al massimo
pub const MAX_GROUP_SETS: usize = 4;

/// "Scena › Fonte" (o "Scena > Fonte") -> "Scena", come parseItem in obs.ts
fn item_container(item: &str) -> Option<&str> {
    let (scene, source) = item.split_once('›').or_else(|| item.split_once('>'))?;
    let scene = scene.trim();
    (!scene.is_empty() && !source.trim().is_empty()).then_some(scene)
}

/// Le scene su cui agisce un set: le scene extra, quelle delle azioni e i contenitori degli elementi.
fn set_scenes(actions: &BTreeMap<u32, GroupAction>, extra: &[String]) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let all = extra.iter().map(|s| s.trim())
        .chain(actions.values().filter_map(|a| a.scene.as_deref()))
        .chain(actions.values().flat_map(|a| a.items.iter().filter_map(|i| item_container(i))));
    for s in all {
        if !s.is_empty() && !out.iter().any(|o| o == s) {
            out.push(s.to_string());
        }
    }
    out
}

impl GroupConfig {
    /// Scene che più set del gruppo userebbero (rifiutate: la scena dice quale set agisce).
    pub fn shared_scenes(&self) -> Vec<String> {
        let mut seen: HashSet<String> = HashSet::new();
        let mut shared: Vec<String> = vec![];
        let sets = std::iter::once(set_scenes(&self.actions, &self.extra_scenes))
            .chain(self.sets.iter().map(|s| set_scenes(&s.actions, &s.extra_scenes)));
        for scenes in sets {
            for s in scenes {
                if !seen.insert(s.clone()) && !shared.contains(&s) {
                    shared.push(s);
                }
            }
        }
        shared
    }
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct Member {
    pub track: String,
    pub label: String,
}

/// Prova di riempimento (come il "test fill" di guestroom): le uscite oltre le persone in onda
/// prendono chi ci viene mandato, anche la stessa persona su più uscite, per provare da soli le
/// scene con più persone. Finisce quando cambiano i membri, quando si svuota un'uscita della
/// prova o con la × sotto il gruppo.
#[derive(Clone, Debug, Default)]
struct TestFill {
    /// membri connessi quando è iniziata: se cambiano, la prova finisce
    sig: Vec<String>,
    /// posizione nel gruppo (indice di uscita) -> (traccia, nome)
    cells: BTreeMap<usize, (String, String)>,
}

#[derive(Default)]
pub struct Groups {
    pub configs: Vec<GroupConfig>,
    pub members: HashMap<u32, Vec<Member>>,
    /// tolti a mano da un gruppo: l'aggiunta automatica non li riaggiunge
    excluded: HashSet<String>,
    fills: HashMap<u32, TestFill>,
    /// Duplicati: uscita di un gruppo -> chi ci mostriamo anche (già su un'altra uscita).
    /// Solo su uscite libere: non cambia l'ordine del gruppo; sparisce quando l'uscita si svuota,
    /// riceve un'altra sorgente, ci arriva un membro o la persona esce di scena.
    dups: BTreeMap<u32, (String, String)>,
}

/// Esito di "manda questa persona su questa uscita del gruppo".
#[derive(Debug, PartialEq)]
pub enum Placed {
    Done,
    /// uscita libera oltre le persone in onda: va chiesto se riempire fino a lì (prova)
    AskFill { landed: u32 },
}

pub(crate) fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// Id più alto mai usato per un'uscita: l'ultimo assegnato, quelli esistenti e quelli
/// ancora citati da vecchi gruppi. Il prossimo id è questo + 1, così un id non viene mai riusato.
pub fn highest_id(last: u32, existing: &[u32], stale: &[u32]) -> u32 {
    existing.iter().chain(stale).copied().fold(last, u32::max)
}

/// Posizione nell'elenco di precedenza (più bassa = più importante); fuori elenco = usize::MAX.
/// "Mario" vale anche per "Mario Rossi" (inizio di parola); maiuscole e spazi non contano.
pub fn rank(priority: &[String], label: &str) -> usize {
    let l = norm(label);
    priority
        .iter()
        .map(|p| norm(p))
        .position(|p| !p.is_empty() && (l == p || l.starts_with(&format!("{p} "))))
        .unwrap_or(usize::MAX)
}

impl Groups {
    pub fn from_configs(configs: Vec<GroupConfig>) -> Self {
        Self { configs, members: HashMap::new(), excluded: HashSet::new(), fills: HashMap::new(), dups: BTreeMap::new() }
    }

    pub fn group_of_slot(&self, slot: u32) -> Option<u32> {
        self.configs.iter().find(|g| g.slots.contains(&slot)).map(|g| g.id)
    }

    fn cfg(&self, gid: u32) -> Option<&GroupConfig> {
        self.configs.iter().find(|g| g.id == gid)
    }

    /// indici (in members) dei membri connessi, in ordine
    fn live_idx(&self, gid: u32, is_live: &dyn Fn(&str) -> bool) -> Vec<usize> {
        self.members
            .get(&gid)
            .map(|m| m.iter().enumerate().filter(|(_, m)| is_live(&m.track)).map(|(i, _)| i).collect())
            .unwrap_or_default()
    }

    /// Cosa mostra ogni uscita di un gruppo: il membro connesso di quel posto, altrimenti la
    /// prova di riempimento, altrimenti un duplicato.
    fn cells(&self, g: &GroupConfig, is_live: &dyn Fn(&str) -> bool) -> Vec<(u32, Option<String>)> {
        let live = self.live_idx(g.id, is_live);
        let members = self.members.get(&g.id);
        let test = self.fills.get(&g.id);
        g.slots
            .iter()
            .enumerate()
            .map(|(i, slot)| {
                let t = live
                    .get(i)
                    .and_then(|&k| members.map(|m| m[k].track.clone()))
                    // oltre le persone in onda: le uscite della prova di riempimento
                    .or_else(|| test.and_then(|f| f.cells.get(&i)).filter(|(t, _)| is_live(t)).map(|(t, _)| t.clone()))
                    .or_else(|| self.dups.get(slot).filter(|(t, _)| is_live(t)).map(|(t, _)| t.clone()));
                (*slot, t)
            })
            .collect()
    }

    /// Cosa deve mostrare ogni slot dei gruppi.
    pub fn fill(&self, is_live: &dyn Fn(&str) -> bool) -> Vec<(u32, Option<String>)> {
        self.configs.iter().flat_map(|g| self.cells(g, is_live)).collect()
    }

    /// Numero di partecipanti che contano per le azioni OBS (0..=slot): le uscite che mostrano
    /// qualcuno, comprese prova di riempimento e duplicati.
    pub fn count(&self, gid: u32, is_live: &dyn Fn(&str) -> bool) -> u32 {
        self.cfg(gid).map(|g| self.cells(g, is_live).iter().filter(|(_, t)| t.is_some()).count() as u32).unwrap_or(0)
    }

    /// Duplica `track` sull'uscita libera `slot` di un gruppo. Ritorna false se l'uscita non è
    /// di un gruppo o non è libera (lì c'è un membro: si usa place, che scambia o inserisce).
    pub fn set_dup(&mut self, slot: u32, track: &str, label: &str, is_live: &dyn Fn(&str) -> bool) -> bool {
        let Some(g) = self.configs.iter().find(|g| g.slots.contains(&slot)) else { return false };
        let pos = g.slots.iter().position(|s| *s == slot).unwrap_or(0);
        if pos < self.live_idx(g.id, is_live).len() {
            return false;
        }
        // una cella della prova su quell'uscita lascia il posto al duplicato
        if let Some(f) = self.fills.get_mut(&g.id) {
            f.cells.remove(&pos);
        }
        self.dups.insert(slot, (track.into(), label.into()));
        true
    }

    /// Toglie il duplicato da un'uscita. Ritorna true se c'era.
    pub fn clear_dup(&mut self, slot: u32) -> bool {
        self.dups.remove(&slot).is_some()
    }

    /// Uscite che mostrano un duplicato.
    pub fn dup_slots(&self) -> Vec<u32> {
        self.dups.keys().copied().collect()
    }

    /// Duplicati che non valgono più: persona uscita di scena, uscita tolta dal gruppo, oppure un
    /// membro arrivato su quel posto (il gruppo ha la precedenza). Ritorna true se ne è sparito uno.
    pub fn expire_dups(&mut self, is_live: &dyn Fn(&str) -> bool) -> bool {
        let before = self.dups.len();
        let keep: Vec<u32> = self
            .dups
            .iter()
            .filter(|(slot, (t, _))| {
                is_live(t)
                    && self.configs.iter().find(|g| g.slots.contains(slot)).is_some_and(|g| {
                        let pos = g.slots.iter().position(|s| s == *slot).unwrap_or(0);
                        pos >= self.live_idx(g.id, is_live).len()
                    })
            })
            .map(|(s, _)| *s)
            .collect();
        self.dups.retain(|s, _| keep.contains(s));
        self.dups.len() != before
    }

    fn live_sig(&self, gid: u32, is_live: &dyn Fn(&str) -> bool) -> Vec<String> {
        let members = self.members.get(&gid);
        self.live_idx(gid, is_live).iter().filter_map(|&k| members.map(|m| m[k].track.clone())).collect()
    }

    /// La prova di riempimento finisce quando cambiano i membri connessi. Ritorna true se ne è finita una.
    pub fn expire_fills(&mut self, is_live: &dyn Fn(&str) -> bool) -> bool {
        let ended: Vec<u32> = self
            .fills
            .iter()
            .filter(|(gid, f)| f.sig != self.live_sig(**gid, is_live))
            .map(|(gid, _)| *gid)
            .collect();
        for gid in &ended {
            self.fills.remove(gid);
        }
        !ended.is_empty()
    }

    /// × sotto il gruppo: termina la prova di riempimento.
    pub fn end_fill(&mut self, gid: u32) -> bool {
        self.fills.remove(&gid).is_some()
    }

    /// Manda qualcuno su un'uscita del gruppo, con le regole della prova di riempimento:
    /// - prova in corso e uscita oltre le persone in onda: la prende subito (anche se è già altrove);
    /// - uscita libera oltre la prima libera: senza `fill` chiede (AskFill); con `fill` avvia la
    ///   prova, mettendo la persona lì e sulle uscite libere prima;
    /// - altrimenti le regole normali (scambio / inserimento / in coda).
    pub fn place_or_fill(
        &mut self,
        gid: u32,
        slot: u32,
        track: &str,
        label: &str,
        fill: bool,
        is_live: &dyn Fn(&str) -> bool,
    ) -> Placed {
        let Some(g) = self.cfg(gid) else { return Placed::Done };
        let Some(pos) = g.slots.iter().position(|s| *s == slot) else { return Placed::Done };
        let n = g.slots.len();
        let first_free = g.slots.get(self.live_idx(gid, is_live).len().min(n)).copied();
        let live_n = self.live_idx(gid, is_live).len();
        if pos >= live_n {
            if let Some(f) = self.fills.get_mut(&gid) {
                f.cells.insert(pos, (track.into(), label.into()));
                return Placed::Done;
            }
            if pos > live_n {
                if !fill {
                    return Placed::AskFill { landed: first_free.unwrap_or(slot) };
                }
                let sig = self.live_sig(gid, is_live);
                let cells = (live_n..=pos).map(|i| (i, (track.to_string(), label.to_string()))).collect();
                self.fills.insert(gid, TestFill { sig, cells });
                return Placed::Done;
            }
        }
        self.place(gid, slot, track, label, is_live);
        Placed::Done
    }

    /// Manda `track` sullo slot `slot` del gruppo.
    pub fn place(&mut self, gid: u32, slot: u32, track: &str, label: &str, is_live: &dyn Fn(&str) -> bool) {
        let Some(pos) = self.cfg(gid).and_then(|g| g.slots.iter().position(|s| *s == slot)) else { return };
        self.excluded.remove(track);
        // una persona sta in un solo gruppo
        for (other, m) in self.members.iter_mut() {
            if *other != gid {
                m.retain(|m| m.track != track);
            }
        }
        let live = self.live_idx(gid, is_live);
        let members = self.members.entry(gid).or_default();
        let holder = live.get(pos).copied();
        if let Some(me) = members.iter().position(|m| m.track == track) {
            // già membro: scambio con chi occupa lo slot (se c'è e non è lui)
            match holder {
                Some(h) if h != me => members.swap(h, me),
                Some(_) => {}
                None => {
                    // slot vuoto oltre i presenti: va in fondo (il gruppo non lascia buchi)
                    let m = members.remove(me);
                    members.push(m);
                }
            }
        } else {
            let m = Member { track: track.into(), label: label.into() };
            match holder {
                Some(h) => members.insert(h, m), // inserito lì, gli altri scalano
                None => members.push(m),
            }
        }
    }

    /// Sposta un membro di un posto tra i membri connessi (delta -1 = verso la prima uscita):
    /// si scambia con il vicino connesso, gli assenti restano dove sono. Ritorna true se si è mosso.
    pub fn move_member(&mut self, gid: u32, track: &str, delta: i32, is_live: &dyn Fn(&str) -> bool) -> bool {
        let live = self.live_idx(gid, is_live);
        let Some(members) = self.members.get_mut(&gid) else { return false };
        let Some(p) = live.iter().position(|&k| members[k].track == track) else { return false };
        let q = p as i32 + delta.signum();
        if delta == 0 || q < 0 || q >= live.len() as i32 {
            return false;
        }
        members.swap(live[p], live[q as usize]);
        true
    }

    pub fn remove(&mut self, gid: u32, track: &str) {
        if let Some(m) = self.members.get_mut(&gid) {
            m.retain(|m| m.track != track);
        }
        self.excluded.insert(track.into());
    }

    fn is_member(&self, track: &str) -> bool {
        self.members.values().flatten().any(|m| m.track == track)
    }

    /// Inserisce davanti al primo membro con meno precedenza; altrimenti in fondo.
    fn insert_ranked(&mut self, gid: u32, m: Member, priority: &[String]) {
        let r = rank(priority, &m.label);
        let members = self.members.entry(gid).or_default();
        match members.iter().position(|x| rank(priority, &x.label) > r) {
            Some(i) => members.insert(i, m),
            None => members.push(m),
        }
    }

    /// Aggiunta automatica nel gruppo gid (il primo): live = webcam in onda (id, nome) in ordine di arrivo.
    pub fn auto_add(&mut self, gid: u32, live: &[(String, String)], priority: &[String]) -> bool {
        if self.cfg(gid).is_none() {
            return false;
        }
        let mut changed = false;
        for (id, label) in live {
            if self.is_member(id) || self.excluded.contains(id) {
                continue;
            }
            self.insert_ranked(gid, Member { track: id.clone(), label: label.clone() }, priority);
            changed = true;
        }
        changed
    }

    /// Elenco di precedenza cambiato: riordina i membri una volta (ordinamento stabile).
    pub fn sort_by_priority(&mut self, priority: &[String]) {
        for m in self.members.values_mut() {
            m.sort_by_key(|x| rank(priority, &x.label));
        }
    }

    /// Svuota uno slot del gruppo: chi c'era esce dal gruppo, gli altri salgono.
    pub fn clear_slot(&mut self, gid: u32, slot: u32, is_live: &dyn Fn(&str) -> bool) {
        let Some(pos) = self.cfg(gid).and_then(|g| g.slots.iter().position(|s| *s == slot)) else { return };
        let live = self.live_idx(gid, is_live);
        // svuotare un'uscita della prova di riempimento termina la prova
        if pos >= live.len() && self.fills.get(&gid).is_some_and(|f| f.cells.contains_key(&pos)) {
            self.fills.remove(&gid);
            return;
        }
        if let (Some(&k), Some(m)) = (live.get(pos), self.members.get_mut(&gid)) {
            let gone = m.remove(k);
            self.excluded.insert(gone.track);
        }
    }

    /// Ospite riconnesso (nuova traccia, stesso nome): riprende il suo posto.
    /// `live` = tracce video connesse (id, nome). Ritorna true se qualcosa è cambiato.
    pub fn rebind(&mut self, live: &[(String, String)], priority: &[String]) -> bool {
        let used: Vec<String> = self.members.values().flatten().map(|m| m.track.clone()).collect();
        let is_live = |t: &str| live.iter().any(|(id, _)| id == t);
        let mut taken: Vec<String> = vec![];
        let mut changed = false;
        for m in self.members.values_mut().flatten() {
            if is_live(&m.track) {
                continue;
            }
            if let Some((id, _)) = live
                .iter()
                .find(|(id, label)| *label == m.label && !used.contains(id) && !taken.contains(id))
            {
                m.track = id.clone();
                taken.push(id.clone());
                changed = true;
            }
        }
        // aggiorna i nomi dei membri connessi (possono arrivare dopo la traccia);
        // se così cambia la precedenza, il membro viene riposizionato
        let mut moved: Vec<(u32, Member)> = vec![];
        for (gid, members) in self.members.iter_mut() {
            let mut i = 0;
            while i < members.len() {
                let m = &mut members[i];
                if let Some((_, label)) = live.iter().find(|(id, _)| *id == m.track) {
                    if m.label != *label {
                        let before = rank(priority, &m.label);
                        m.label = label.clone();
                        changed = true;
                        if rank(priority, label) != before {
                            moved.push((*gid, members.remove(i)));
                            continue;
                        }
                    }
                }
                i += 1;
            }
        }
        for (gid, m) in moved {
            self.insert_ranked(gid, m, priority);
        }
        changed
    }

    /// Crea (id 0) o aggiorna un gruppo. Uno slot appartiene al massimo a un gruppo.
    pub fn save(&mut self, mut g: GroupConfig, valid: &[u32]) -> Result<u32, String> {
        let name = g.name.trim().to_string();
        if name.is_empty() {
            return Err("il gruppo deve avere un nome".into());
        }
        g.name = name;
        if g.slots.is_empty() {
            return Err("il gruppo deve avere almeno uno slot".into());
        }
        let mut seen = vec![];
        for s in &g.slots {
            if !valid.contains(s) || seen.contains(s) {
                return Err(format!("slot {s} non valido"));
            }
            seen.push(*s);
            if let Some(other) = self.configs.iter().find(|o| o.id != g.id && o.slots.contains(s)) {
                return Err(format!("lo slot {s} è già nel gruppo \"{}\"", other.name));
            }
        }
        let n = g.slots.len() as u32;
        g.actions.retain(|k, a| *k <= n && (a.scene.is_some() || !a.items.is_empty()));
        g.extra_scenes.retain(|s| !s.trim().is_empty());
        if g.sets.len() > MAX_GROUP_SETS {
            return Err(format!("al massimo {MAX_GROUP_SETS} set di scene oltre a quello principale"));
        }
        let mut names: Vec<String> = vec![];
        for set in g.sets.iter_mut() {
            set.name = set.name.trim().to_string();
            if set.name.is_empty() {
                return Err("ogni set di scene deve avere un nome".into());
            }
            let key = set.name.to_lowercase();
            if names.contains(&key) {
                return Err(format!("due set di scene si chiamano \"{}\"", set.name));
            }
            names.push(key);
            set.actions.retain(|k, a| *k <= n && (a.scene.is_some() || !a.items.is_empty()));
            set.extra_scenes.retain(|s| !s.trim().is_empty());
        }
        let shared = g.shared_scenes();
        if !shared.is_empty() {
            return Err(format!("una scena appartiene a un solo set: {}", shared.join(", ")));
        }
        if g.id == 0 {
            g.id = self.configs.iter().map(|c| c.id).max().unwrap_or(0) + 1;
            let id = g.id;
            self.configs.push(g);
            Ok(id)
        } else if let Some(c) = self.configs.iter_mut().find(|c| c.id == g.id) {
            *c = g;
            Ok(c.id)
        } else {
            Err("gruppo inesistente".into())
        }
    }

    /// Config caricata: toglie dai gruppi le uscite che non esistono più (es. gruppi della versione
    /// a 8 slot fissi), altrimenti una nuova uscita con lo stesso id ci finirebbe dentro.
    /// Ritorna gli id tolti.
    pub fn drop_missing_slots(&mut self, existing: &[u32]) -> Vec<u32> {
        let mut stale: Vec<u32> = self
            .configs
            .iter()
            .flat_map(|g| g.slots.iter().copied())
            .filter(|s| !existing.contains(s))
            .collect();
        stale.sort_unstable();
        stale.dedup();
        for s in &stale {
            self.remove_slot(*s);
        }
        stale
    }

    /// Uscita eliminata: esce dai gruppi; un gruppo rimasto senza uscite viene eliminato.
    pub fn remove_slot(&mut self, slot: u32) {
        for g in self.configs.iter_mut() {
            g.slots.retain(|s| *s != slot);
            let n = g.slots.len() as u32;
            g.actions.retain(|k, _| *k <= n);
            for set in g.sets.iter_mut() {
                set.actions.retain(|k, _| *k <= n);
            }
        }
        let empty: Vec<u32> = self.configs.iter().filter(|g| g.slots.is_empty()).map(|g| g.id).collect();
        for id in empty {
            self.delete(id);
        }
    }

    pub fn delete(&mut self, gid: u32) {
        self.configs.retain(|g| g.id != gid);
        self.members.remove(&gid);
        self.fills.remove(&gid);
    }

    pub fn view(&self, is_live: &dyn Fn(&str) -> bool) -> Value {
        let groups: Vec<Value> = self
            .configs
            .iter()
            .map(|g| {
                let members = self.members.get(&g.id).cloned().unwrap_or_default();
                let mut live_n = 0usize;
                let list: Vec<Value> = members
                    .iter()
                    .map(|m| {
                        let place = if !is_live(&m.track) {
                            "away"
                        } else {
                            live_n += 1;
                            if live_n <= g.slots.len() { "on" } else { "standby" }
                        };
                        let slot = if place == "on" { Some(g.slots[live_n - 1]) } else { None };
                        json!({ "track": m.track, "label": m.label, "place": place, "slot": slot })
                    })
                    .collect();
                // prova di riempimento: uscite riempite (oltre le persone in onda) e chi c'è
                let live = self.live_idx(g.id, is_live).len();
                let test = self.fills.get(&g.id).map(|f| {
                    let cells: Vec<(&usize, &(String, String))> =
                        f.cells.iter().filter(|(i, _)| **i >= live && **i < g.slots.len()).collect();
                    // ogni nome una volta sola, nell'ordine delle uscite
                    let mut names: Vec<&str> = vec![];
                    for (_, (_, l)) in &cells {
                        if !names.contains(&l.as_str()) {
                            names.push(l.as_str());
                        }
                    }
                    json!({
                        "slots": cells.iter().map(|(i, _)| g.slots[**i]).collect::<Vec<u32>>(),
                        "names": names,
                    })
                });
                json!({
                    "id": g.id,
                    "name": g.name,
                    "slots": g.slots,
                    "actions": g.actions,
                    "extraScenes": g.extra_scenes,
                    "sets": g.sets,
                    "members": list,
                    "count": self.count(g.id, is_live),
                    "testFill": test,
                })
            })
            .collect();
        Value::Array(groups)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grp(slots: &[u32]) -> Groups {
        let mut g = Groups::default();
        g.save(GroupConfig { id: 0, name: "Panel".into(), slots: slots.to_vec(), actions: Default::default(), extra_scenes: vec![], sets: vec![] }, &[1, 2, 3, 4, 5, 6, 7, 8])
            .unwrap();
        g
    }
    fn order(g: &Groups) -> Vec<String> {
        g.members[&1].iter().map(|m| m.track.clone()).collect()
    }
    const ALL: &dyn Fn(&str) -> bool = &|_| true;
    const ALL_SLOTS: &[u32] = &[1, 2, 3, 4, 5, 6, 7, 8];

    #[test]
    fn riempie_in_ordine_e_sale_quando_uno_esce() {
        let mut g = grp(&[3, 1]);
        g.place(1, 3, "a", "A", ALL);
        g.place(1, 1, "b", "B", ALL);
        assert_eq!(g.fill(ALL), vec![(3, Some("a".into())), (1, Some("b".into()))]);
        let no_a = |t: &str| t != "a";
        assert_eq!(g.fill(&no_a), vec![(3, Some("b".into())), (1, None)]);
        assert_eq!(g.count(1, &no_a), 1);
    }

    #[test]
    fn nuovo_inserito_e_ultimo_in_standby() {
        let mut g = grp(&[1, 2]);
        g.place(1, 1, "a", "A", ALL);
        g.place(1, 2, "b", "B", ALL);
        g.place(1, 1, "c", "C", ALL);
        assert_eq!(order(&g), vec!["c", "a", "b"]);
        let v = g.view(ALL);
        assert_eq!(v[0]["members"][2]["place"], "standby");
        assert_eq!(g.count(1, ALL), 2);
    }

    #[test]
    fn membro_si_scambia() {
        let mut g = grp(&[1, 2, 3]);
        for (s, t) in [(1, "a"), (2, "b"), (3, "c")] {
            g.place(1, s, t, t, ALL);
        }
        g.place(1, 1, "c", "c", ALL);
        assert_eq!(order(&g), vec!["c", "b", "a"]);
    }

    #[test]
    fn slot_vuoto_oltre_i_presenti_accoda_senza_buchi() {
        let mut g = grp(&[1, 2, 3]);
        g.place(1, 3, "a", "A", ALL);
        assert_eq!(g.fill(ALL)[0], (1, Some("a".into())));
    }

    #[test]
    fn svuotare_slot_toglie_dal_gruppo() {
        let mut g = grp(&[1, 2]);
        g.place(1, 1, "a", "A", ALL);
        g.place(1, 2, "b", "B", ALL);
        g.clear_slot(1, 1, ALL);
        assert_eq!(order(&g), vec!["b"]);
    }

    #[test]
    fn assente_ripreso_per_nome() {
        let mut g = grp(&[1, 2]);
        g.place(1, 1, "a", "Mario", ALL);
        g.place(1, 2, "b", "Giulia", ALL);
        let live = vec![("b".to_string(), "Giulia".to_string()), ("a2".to_string(), "Mario".to_string())];
        assert!(g.rebind(&live, &[]));
        assert_eq!(order(&g), vec!["a2", "b"]);
    }

    #[test]
    fn uno_slot_in_un_solo_gruppo() {
        let mut g = grp(&[1, 2]);
        let r = g.save(GroupConfig { id: 0, name: "Altro".into(), slots: vec![2, 3], actions: Default::default(), extra_scenes: vec![], sets: vec![] }, &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert!(r.unwrap_err().contains("già nel gruppo"));
    }

    #[test]
    fn una_persona_in_un_solo_gruppo() {
        let mut g = grp(&[1]);
        g.save(GroupConfig { id: 0, name: "B".into(), slots: vec![2], actions: Default::default(), extra_scenes: vec![], sets: vec![] }, &[1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
        g.place(1, 1, "a", "A", ALL);
        g.place(2, 2, "a", "A", ALL);
        assert!(g.members[&1].is_empty());
        assert_eq!(g.members[&2].len(), 1);
    }

    #[test]
    fn uscita_eliminata_esce_dal_gruppo_e_gruppo_vuoto_sparisce() {
        let mut g = grp(&[1, 2]);
        g.place(1, 1, "a", "A", ALL);
        g.remove_slot(1);
        assert_eq!(g.configs[0].slots, vec![2]);
        assert_eq!(g.fill(ALL), vec![(2, Some("a".into()))]);
        g.remove_slot(2);
        assert!(g.configs.is_empty());
    }

    fn live(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    #[test]
    fn aggiunta_automatica_in_ordine_di_arrivo() {
        let mut g = grp(&[1, 2]);
        assert!(g.auto_add(1, &live(&[("a", "Anna"), ("b", "Bruno"), ("c", "Carla")]), &[]));
        assert_eq!(order(&g), vec!["a", "b", "c"]);
        assert_eq!(g.view(ALL)[0]["members"][2]["place"], "standby");
        assert!(!g.auto_add(1, &live(&[("a", "Anna"), ("b", "Bruno"), ("c", "Carla")]), &[]));
    }

    #[test]
    fn precedenza_passa_davanti_e_manda_in_attesa_l_ultimo() {
        let mut g = grp(&[1, 2]);
        let p = vec!["Mario".to_string(), "Giulia".to_string()];
        g.auto_add(1, &live(&[("a", "Anna"), ("b", "Bruno")]), &p);
        g.auto_add(1, &live(&[("g", "Giulia Bianchi")]), &p);
        g.auto_add(1, &live(&[("m", "mario  rossi")]), &p);
        assert_eq!(order(&g), vec!["m", "g", "a", "b"]);
        assert_eq!(g.fill(ALL), vec![(1, Some("m".into())), (2, Some("g".into()))]);
    }

    #[test]
    fn tolto_a_mano_non_viene_riaggiunto() {
        let mut g = grp(&[1, 2]);
        g.auto_add(1, &live(&[("a", "Anna"), ("b", "Bruno")]), &[]);
        g.clear_slot(1, 1, ALL);
        g.remove(1, "b");
        assert!(!g.auto_add(1, &live(&[("a", "Anna"), ("b", "Bruno")]), &[]));
        g.place(1, 1, "a", "Anna", ALL);
        assert_eq!(order(&g), vec!["a"]);
    }

    #[test]
    fn nome_arrivato_dopo_riposiziona_per_precedenza() {
        let mut g = grp(&[1, 2]);
        let p = vec!["Mario".to_string()];
        g.auto_add(1, &live(&[("a", "Anna"), ("x", "Ospite 3")]), &p);
        assert_eq!(order(&g), vec!["a", "x"]);
        g.rebind(&live(&[("a", "Anna"), ("x", "Mario")]), &p);
        assert_eq!(order(&g), vec!["x", "a"]);
    }

    #[test]
    fn elenco_cambiato_riordina_una_volta() {
        let mut g = grp(&[1, 2, 3]);
        g.auto_add(1, &live(&[("a", "Anna"), ("b", "Bruno"), ("c", "Carla")]), &[]);
        g.sort_by_priority(&["Carla".to_string(), "Anna".to_string()]);
        assert_eq!(order(&g), vec!["c", "a", "b"]);
    }
#[test]
    fn gruppo_della_versione_a_8_slot_perde_le_uscite_inesistenti() {
        let mut g = Groups::from_configs(vec![GroupConfig {
            id: 1, name: "Camera".into(), slots: vec![1, 2, 3, 4, 5, 6], actions: Default::default(), extra_scenes: vec![], sets: vec![],
        }]);
        let stale = g.drop_missing_slots(&[1, 2, 3, 4]);
        assert_eq!(stale, vec![5, 6]);
        assert_eq!(g.configs[0].slots, vec![1, 2, 3, 4]);
        // la prossima uscita non riusa 5 o 6
        let next = highest_id(0, &[1, 2, 3, 4], &stale) + 1;
        assert_eq!(next, 7);
        assert!(g.group_of_slot(next).is_none());
    }

    #[test]
    fn gruppo_con_sole_uscite_inesistenti_sparisce() {
        let mut g = Groups::from_configs(vec![GroupConfig {
            id: 1, name: "Vecchio".into(), slots: vec![7, 8], actions: Default::default(), extra_scenes: vec![], sets: vec![],
        }]);
        g.drop_missing_slots(&[1, 2, 3, 4]);
        assert!(g.configs.is_empty());
    }

    #[test]
    fn id_mai_riusati() {
        // ultima assegnata 9 (poi eliminata): anche se ora esistono solo 1..4 si riparte da 10
        assert_eq!(highest_id(9, &[1, 2, 3, 4], &[]) + 1, 10);
        assert_eq!(highest_id(0, &[], &[]) + 1, 1);
    }
#[test]
    fn uscita_oltre_la_prima_libera_chiede_la_prova() {
        let mut g = grp(&[1, 2, 3, 4]);
        g.place(1, 1, "a", "Anna", ALL);
        // prima libera (Cam 2): entra subito
        assert_eq!(g.place_or_fill(1, 2, "b", "Bruno", false, ALL), Placed::Done);
        // Cam 4 con 2 persone in onda: chiede, e direbbe che andrebbe su Cam 3
        assert_eq!(g.place_or_fill(1, 4, "c", "Carla", false, ALL), Placed::AskFill { landed: 3 });
        assert_eq!(order(&g), vec!["a", "b"]);
    }

    #[test]
    fn prova_riempie_fino_all_uscita_scelta_con_la_stessa_persona() {
        let mut g = grp(&[1, 2, 3, 4]);
        g.place(1, 1, "a", "Anna", ALL);
        assert_eq!(g.place_or_fill(1, 4, "a", "Anna", true, ALL), Placed::Done);
        // Anna è membro su Cam 1 e riempie anche Cam 2..4: stessa persona su più uscite
        assert_eq!(g.fill(ALL), vec![(1, Some("a".into())), (2, Some("a".into())), (3, Some("a".into())), (4, Some("a".into()))]);
        assert_eq!(g.count(1, ALL), 4);
        assert_eq!(g.view(ALL)[0]["testFill"]["slots"], json!([2, 3, 4]));
        assert_eq!(order(&g), vec!["a"]); // i membri non cambiano
    }

    #[test]
    fn durante_la_prova_le_uscite_oltre_prendono_chiunque_anche_alternati() {
        let mut g = grp(&[1, 2, 3, 4]);
        g.place(1, 1, "a", "Anna", ALL);
        g.place_or_fill(1, 3, "b", "Bruno", true, ALL); // Cam 2 e 3 = Bruno
        assert_eq!(g.place_or_fill(1, 2, "a", "Anna", false, ALL), Placed::Done); // senza chiedere
        assert_eq!(g.place_or_fill(1, 4, "b", "Bruno", false, ALL), Placed::Done);
        let f: Vec<Option<String>> = g.fill(ALL).into_iter().map(|(_, t)| t).collect();
        assert_eq!(f, vec![Some("a".into()), Some("a".into()), Some("b".into()), Some("b".into())]);
    }

    #[test]
    fn prova_finisce_svuotando_un_uscita_della_prova_o_con_la_x() {
        let mut g = grp(&[1, 2, 3]);
        g.place(1, 1, "a", "Anna", ALL);
        g.place_or_fill(1, 3, "a", "Anna", true, ALL);
        g.clear_slot(1, 2, ALL); // uscita della prova: finisce la prova, Anna resta membro
        assert_eq!(g.fill(ALL), vec![(1, Some("a".into())), (2, None), (3, None)]);
        assert_eq!(order(&g), vec!["a"]);
        g.place_or_fill(1, 3, "a", "Anna", true, ALL);
        assert!(g.end_fill(1));
        assert_eq!(g.count(1, ALL), 1);
    }

    #[test]
    fn prova_finisce_quando_cambiano_i_membri() {
        let mut g = grp(&[1, 2, 3]);
        g.place(1, 1, "a", "Anna", ALL);
        g.place_or_fill(1, 3, "a", "Anna", true, ALL);
        assert!(!g.expire_fills(ALL));
        g.place(1, 2, "b", "Bruno", ALL); // nuovo membro
        assert!(g.expire_fills(ALL));
        assert_eq!(g.fill(ALL), vec![(1, Some("a".into())), (2, Some("b".into())), (3, None)]);
    }

    fn act(scene: Option<&str>, items: &[&str]) -> GroupAction {
        GroupAction { scene: scene.map(Into::into), items: items.iter().map(|s| s.to_string()).collect() }
    }

    fn with_sets(sets: Vec<GroupSceneSet>) -> GroupConfig {
        GroupConfig {
            id: 0, name: "Camera".into(), slots: vec![1, 2, 3],
            actions: BTreeMap::from([(1, act(Some("Cam 1"), &[])), (2, act(Some("Cam 2"), &["Cam 2 › Ospite"]))]),
            extra_scenes: vec![], sets,
        }
    }

    fn reaction(name: &str, actions: BTreeMap<u32, GroupAction>) -> GroupSceneSet {
        GroupSceneSet { name: name.into(), actions, extra_scenes: vec![] }
    }

    #[test]
    fn set_di_scene_salvati_e_ripuliti() {
        let mut g = Groups::default();
        let set = reaction(" Reaction ", BTreeMap::from([(2, act(Some("Reaction 2"), &[])), (3, act(None, &[])), (9, act(Some("R9"), &[]))]));
        let id = g.save(with_sets(vec![set]), ALL_SLOTS).unwrap();
        let s = &g.cfg(id).unwrap().sets[0];
        assert_eq!(s.name, "Reaction");
        assert_eq!(s.actions.keys().copied().collect::<Vec<_>>(), vec![2]); // vuota e oltre le uscite: tolte
    }

    #[test]
    fn una_scena_appartiene_a_un_solo_set() {
        let mut g = Groups::default();
        // "Cam 2" è già del set principale (scena dell'azione e contenitore dell'elemento)
        let set = reaction("Reaction", BTreeMap::from([(1, act(None, &["Cam 2 > Overlay"]))]));
        let e = g.save(with_sets(vec![set]), ALL_SLOTS).unwrap_err();
        assert!(e.contains("Cam 2"), "{e}");
        let set = GroupSceneSet { name: "R".into(), actions: Default::default(), extra_scenes: vec!["Cam 1".into()] };
        assert!(g.save(with_sets(vec![set]), ALL_SLOTS).is_err());
    }

    #[test]
    fn set_con_nomi_doppi_o_vuoti_o_troppi_rifiutati() {
        let mut g = Groups::default();
        let two = vec![reaction("Reaction", Default::default()), reaction("reaction", Default::default())];
        assert!(g.save(with_sets(two), ALL_SLOTS).is_err());
        assert!(g.save(with_sets(vec![reaction("  ", Default::default())]), ALL_SLOTS).is_err());
        let many = (0..=MAX_GROUP_SETS).map(|i| reaction(&format!("S{i}"), Default::default())).collect();
        assert!(g.save(with_sets(many), ALL_SLOTS).is_err());
    }

    #[test]
    fn togliere_un_uscita_ripulisce_anche_i_set() {
        let mut g = Groups::default();
        let set = reaction("Reaction", BTreeMap::from([(3, act(Some("Reaction 3"), &[]))]));
        let id = g.save(with_sets(vec![set]), ALL_SLOTS).unwrap();
        g.remove_slot(3);
        assert!(g.cfg(id).unwrap().sets[0].actions.is_empty());
    }

    #[test]
    fn sposta_un_membro_tra_i_connessi_saltando_gli_assenti() {
        let mut g = grp(&[1, 2, 3]);
        g.place(1, 1, "a", "Anna", ALL);
        g.place(1, 2, "b", "Bruno", ALL);
        g.place(1, 3, "c", "Carla", ALL);
        let no_b: &dyn Fn(&str) -> bool = &|t| t != "b"; // Bruno assente
        assert!(g.move_member(1, "c", -1, no_b)); // Carla scavalca Anna, non l'assente
        assert_eq!(order(&g), vec!["c", "b", "a"]);
        assert!(!g.move_member(1, "c", -1, no_b)); // già prima
        assert!(!g.move_member(1, "b", 1, no_b)); // un assente non si sposta
        assert!(g.move_member(1, "c", 1, ALL));
        assert_eq!(order(&g), vec!["b", "c", "a"]);
    }

    #[test]
    fn duplicato_su_un_uscita_libera_del_gruppo() {
        let mut g = grp(&[1, 2, 3, 4]);
        g.place(1, 1, "a", "Anna", ALL);
        g.place(1, 2, "b", "Bruno", ALL);
        assert!(!g.set_dup(2, "a", "Anna", ALL)); // occupata da Bruno: niente duplicato
        assert!(g.set_dup(4, "a", "Anna", ALL)); // libera, anche non consecutiva
        assert_eq!(g.fill(ALL), vec![(1, Some("a".into())), (2, Some("b".into())), (3, None), (4, Some("a".into()))]);
        assert_eq!(g.count(1, ALL), 3); // le azioni OBS contano le uscite che mostrano qualcuno
        assert_eq!(order(&g), vec!["a", "b"]); // l'ordine del gruppo non cambia
        assert!(g.clear_dup(4));
        assert_eq!(g.count(1, ALL), 2);
    }

    #[test]
    fn duplicato_sparisce_quando_arriva_un_membro_o_la_persona_esce() {
        let mut g = grp(&[1, 2, 3]);
        g.place(1, 1, "a", "Anna", ALL);
        assert!(g.set_dup(2, "a", "Anna", ALL));
        g.place(1, 2, "c", "Carla", ALL); // Carla entra nel gruppo: prende il posto 2
        assert!(g.expire_dups(ALL));
        assert_eq!(g.fill(ALL)[1], (2, Some("c".into())));
        assert!(g.set_dup(3, "a", "Anna", ALL));
        let no_a: &dyn Fn(&str) -> bool = &|t| t != "a";
        assert!(g.expire_dups(no_a)); // Anna esce di scena
        assert!(g.dup_slots().is_empty());
    }
}
