//! Gruppi di output (come i "camera groups" di guestroom.live).
//!
//! Un gruppo = elenco ordinato di slot (ordine di riempimento) + elenco ordinato di membri.
//! Il membro connesso n. 1 va sul primo slot, il 2 sul secondo, ecc.; gli slot avanzati restano vuoti.
//! - Contano solo i membri connessi: se uno esce gli altri salgono subito, anche in onda.
//! - Chi si riconnette con lo stesso nome riprende il suo posto (rebind per nome).
//! - Mandare qualcuno su uno slot del gruppo: un membro si SCAMBIA con chi è lì,
//!   un nuovo arrivato viene INSERITO lì e chi finisce oltre l'ultimo slot va in standby.
//! - Svuotare uno slot del gruppo toglie dal gruppo chi c'era; gli altri salgono.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};

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
}

impl Groups {
    pub fn from_configs(configs: Vec<GroupConfig>) -> Self {
        Self { configs, members: HashMap::new() }
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
    }

    /// Svuota uno slot del gruppo: chi c'era esce dal gruppo, gli altri salgono.
    pub fn clear_slot(&mut self, gid: u32, slot: u32, is_live: &dyn Fn(&str) -> bool) {
        let Some(pos) = self.cfg(gid).and_then(|g| g.slots.iter().position(|s| *s == slot)) else { return };
        let live = self.live_idx(gid, is_live);
        if let (Some(&k), Some(m)) = (live.get(pos), self.members.get_mut(&gid)) {
            m.remove(k);
        }
    }

    /// Ospite riconnesso (nuova traccia, stesso nome): riprende il suo posto.
    /// `live` = tracce video connesse (id, nome). Ritorna true se qualcosa è cambiato.
    pub fn rebind(&mut self, live: &[(String, String)]) -> bool {
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
        // aggiorna i nomi dei membri connessi (il nome arriva dal DOM qualche secondo dopo)
        for m in self.members.values_mut().flatten() {
            if let Some((_, label)) = live.iter().find(|(id, _)| *id == m.track) {
                if m.label != *label {
                    m.label = label.clone();
                    changed = true;
                }
            }
        }
        changed
    }

    /// Crea (id 0) o aggiorna un gruppo. Uno slot appartiene al massimo a un gruppo.
    pub fn save(&mut self, mut g: GroupConfig, slot_count: u32) -> Result<u32, String> {
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
            if !(1..=slot_count).contains(s) || seen.contains(s) {
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
        g.save(GroupConfig { id: 0, name: "Panel".into(), slots: slots.to_vec(), actions: Default::default(), extra_scenes: vec![] }, 8)
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
        assert!(g.rebind(&live));
        assert_eq!(order(&g), vec!["a2", "b"]);
    }

    #[test]
    fn uno_slot_in_un_solo_gruppo() {
        let mut g = grp(&[1, 2]);
        let r = g.save(GroupConfig { id: 0, name: "Altro".into(), slots: vec![2, 3], actions: Default::default(), extra_scenes: vec![] }, 8);
        assert!(r.unwrap_err().contains("già nel gruppo"));
    }

    #[test]
    fn una_persona_in_un_solo_gruppo() {
        let mut g = grp(&[1]);
        g.save(GroupConfig { id: 0, name: "B".into(), slots: vec![2], actions: Default::default(), extra_scenes: vec![] }, 8).unwrap();
        g.place(1, 1, "a", "A", ALL);
        g.place(2, 2, "a", "A", ALL);
        assert!(g.members[&1].is_empty());
        assert_eq!(g.members[&2].len(), 1);
    }
}
