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
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct Member {
    pub track: String,
    pub label: String,
}

#[derive(Default)]
pub struct Groups {
    pub configs: Vec<GroupConfig>,
    pub members: HashMap<u32, Vec<Member>>,
    /// tolti a mano da un gruppo: l'aggiunta automatica non li riaggiunge
    excluded: HashSet<String>,
}

fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
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
        Self { configs, members: HashMap::new(), excluded: HashSet::new() }
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

    /// Cosa deve mostrare ogni slot dei gruppi.
    pub fn fill(&self, is_live: &dyn Fn(&str) -> bool) -> Vec<(u32, Option<String>)> {
        let mut out = vec![];
        for g in &self.configs {
            let live = self.live_idx(g.id, is_live);
            let members = self.members.get(&g.id);
            for (i, slot) in g.slots.iter().enumerate() {
                let t = live.get(i).and_then(|&k| members.map(|m| m[k].track.clone()));
                out.push((*slot, t));
            }
        }
        out
    }

    /// Numero di partecipanti che contano per le azioni OBS (0..=slot).
    pub fn count(&self, gid: u32, is_live: &dyn Fn(&str) -> bool) -> u32 {
        let n = self.cfg(gid).map(|g| g.slots.len()).unwrap_or(0);
        self.live_idx(gid, is_live).len().min(n) as u32
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

    /// Uscita eliminata: esce dai gruppi; un gruppo rimasto senza uscite viene eliminato.
    pub fn remove_slot(&mut self, slot: u32) {
        for g in self.configs.iter_mut() {
            g.slots.retain(|s| *s != slot);
            let n = g.slots.len() as u32;
            g.actions.retain(|k, _| *k <= n);
        }
        let empty: Vec<u32> = self.configs.iter().filter(|g| g.slots.is_empty()).map(|g| g.id).collect();
        for id in empty {
            self.delete(id);
        }
    }

    pub fn delete(&mut self, gid: u32) {
        self.configs.retain(|g| g.id != gid);
        self.members.remove(&gid);
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
                json!({
                    "id": g.id,
                    "name": g.name,
                    "slots": g.slots,
                    "actions": g.actions,
                    "extraScenes": g.extra_scenes,
                    "members": list,
                    "count": self.count(g.id, is_live),
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
        g.save(GroupConfig { id: 0, name: "Panel".into(), slots: slots.to_vec(), actions: Default::default(), extra_scenes: vec![] }, &[1, 2, 3, 4, 5, 6, 7, 8])
            .unwrap();
        g
    }
    fn order(g: &Groups) -> Vec<String> {
        g.members[&1].iter().map(|m| m.track.clone()).collect()
    }
    const ALL: &dyn Fn(&str) -> bool = &|_| true;

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
        let r = g.save(GroupConfig { id: 0, name: "Altro".into(), slots: vec![2, 3], actions: Default::default(), extra_scenes: vec![] }, &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert!(r.unwrap_err().contains("già nel gruppo"));
    }

    #[test]
    fn una_persona_in_un_solo_gruppo() {
        let mut g = grp(&[1]);
        g.save(GroupConfig { id: 0, name: "B".into(), slots: vec![2], actions: Default::default(), extra_scenes: vec![] }, &[1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
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
}
