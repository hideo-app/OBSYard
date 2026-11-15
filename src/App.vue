<script setup lang="ts">
import { nextTick, onMounted, ref, watch } from "vue";
import {
  state, init, videoTracks, audioTracks, slotLayout, setSettings, setObs, studio, addOutput, nextOutputName, setFullRes,
  type Settings, type GroupView, type ObsConfig,
} from "./store";
import { obs, startObs } from "./obs";
import SourceCard from "./components/SourceCard.vue";
import SlotRow from "./components/SlotRow.vue";
import GroupBox from "./components/GroupBox.vue";
import GroupDialog from "./components/GroupDialog.vue";

const error = ref<string | null>(null);
const draft = ref<Settings>({ ...state.settings });
watch(() => state.settings, (s) => (draft.value = { ...s }), { deep: true });
const obsDraft = ref<ObsConfig>({ ...state.obs });
watch(() => state.obs, (o) => (obsDraft.value = { ...o }), { deep: true });

/** undefined = chiuso, null = nuovo gruppo */
const editing = ref<GroupView | null | undefined>(undefined);

onMounted(() =>
  init()
    .then(startObs)
    .catch((e) => (error.value = String(e))),
);

const saveSettings = () =>
  setSettings({ ...draft.value, maxBitrateKbps: Number(draft.value.maxBitrateKbps), maxFps: Number(draft.value.maxFps) });
const saveObs = () => setObs({ ...obsDraft.value });

// "Aggiungi uscita": modulo in linea col nome "Cam N" già proposto e selezionato
const adding = ref(false);
const newName = ref("");
const newInput = ref<HTMLInputElement>();
const startAdd = async () => {
  newName.value = nextOutputName();
  adding.value = true;
  await nextTick();
  newInput.value?.select();
};
const commitAdd = async () => {
  await addOutput(newName.value.trim() || undefined);
  adding.value = false;
};
const toggleDrive = () => setObs({ ...state.obs, drive: !state.obs.drive });

// partecipanti: aggiunta automatica al primo gruppo + precedenza (un nome per riga)
const toggleAutoAdd = () => setSettings({ ...state.settings, autoAdd: !state.settings.autoAdd });
const NL = "\n";
const parseNames = (text: string) => text.split(NL).map((s) => s.trim()).filter(Boolean);
const priorityText = ref(state.settings.priority.join(NL));
// aggiorna il testo quando la precedenza cambia altrove (es. all'avvio), senza toccare quello che stai scrivendo
watch(() => state.settings.priority, (p) => {
  if (p.join(NL) !== parseNames(priorityText.value).join(NL)) priorityText.value = p.join(NL);
});
const savePriority = () => setSettings({ ...state.settings, priority: parseNames(priorityText.value) });

const fullResInfo = (): { cls: string; text: string; title?: string } | null => {
  const s = state.fullRes.status;
  const name = state.fullRes.layout;
  if (s.startsWith("error:")) return { cls: "bad", text: "Layout non scritto", title: s.slice(6) };
  return (
    ({
      off: null,
      "": { cls: "wait", text: "Scrivo il layout…" },
      on: { cls: "ok", text: `Layout "${name}" attivo`, title: "OBSYard lo riscrive a ogni attivazione: una colonna a piena altezza per ogni camera" },
      created: { cls: "wait", text: `Layout "${name}" creato: ricarica lo studio`, title: "StreamYard mostra i layout nuovi dopo un ricaricamento (pulsante Ricarica): il toggle lo attiva da solo" },
      updated: { cls: "wait", text: `Layout "${name}" aggiornato: ricarica lo studio`, title: "Lo studio usa la copia del layout letta all'apertura. Dopo il ricaricamento (pulsante Ricarica) il toggle lo attiva da solo; poi non serve più, finché nessuno lo modifica" },
      missing: { cls: "bad", text: `Layout "${name}" non trovato`, title: "Entra nello studio StreamYard e riattiva il toggle" },
      manual: { cls: "wait", text: "Layout cambiato a mano", title: "OBSYard non lo forza: riattiva il toggle per tornare al layout a piena risoluzione" },
    } as Record<string, { cls: string; text: string; title?: string } | null>)[s] ?? null
  );
};

const obsLabel = () =>
  ({ idle: "OBS: non configurato", connecting: "OBS: connessione…", connected: "OBS connesso", auth: "OBS: password errata", error: "OBS non raggiungibile" })[obs.status];
</script>

<template>
  <header class="top">
    <div class="brand"><span class="dot" />OBSYard</div>
    <span class="pill" :class="state.publisherReady ? 'ok' : 'wait'">
      {{ state.publisherReady ? "StreamYard collegato" : "In attesa di StreamYard…" }}
    </span>
    <span v-if="state.obs.drive" class="pill" :class="obs.status === 'connected' ? 'ok' : 'wait'" :title="obs.error">{{ obsLabel() }}</span>
    <label class="drive" title="I gruppi cambiano scene ed elementi di OBS in base al numero di partecipanti">
      <input type="checkbox" :checked="state.obs.drive" @change="toggleDrive" /> Pilota OBS
    </label>
    <label class="drive" title="Passa al layout personalizzato con tutti i riquadri a piena altezza: gli ospiti inviano alla massima risoluzione. Cambia anche il layout della diretta di StreamYard.">
      <input type="checkbox" :checked="state.fullRes.on" @change="setFullRes(!state.fullRes.on)" /> Ospiti a piena risoluzione
    </label>
    <span v-if="fullResInfo()" class="gpill" :class="fullResInfo()!.cls" :title="fullResInfo()!.title">{{ fullResInfo()!.text }}</span>
    <div class="spacer" />
    <button @click="studio('show')">Mostra StreamYard</button>
    <button class="ghost" @click="studio('reload')">Ricarica</button>
  </header>

  <p v-if="error" class="err">{{ error }}</p>

  <main class="grid">
    <section class="panel">
      <h2>Sorgenti <small>{{ videoTracks.length }} video · {{ audioTracks.length }} audio</small></h2>
      <p v-if="!videoTracks.length" class="empty">
        Nessuna sorgente. Entra nello studio StreamYard: ospiti, camera e schermi condivisi compariranno qui.
      </p>
      <div class="cards">
        <SourceCard v-for="t in videoTracks" :key="t.id" :track="t" />
      </div>
    </section>

    <section class="panel">
      <div class="uhead">
        <h2>Uscite OBS <small>Fonte browser, consigliato 1920×1080</small></h2>
        <label class="bgpick" title="Cosa mostrano le uscite senza video: vuote, ospite fuori onda, camera spenta. Ogni uscita può fare eccezione dal suo menu ⋮">
          Senza video
          <select :value="state.settings.emptyBg" @change="setSettings({ ...state.settings, emptyBg: ($event.target as HTMLSelectElement).value as 'transparent' | 'black' })">
            <option value="transparent">Trasparente</option>
            <option value="black">Nero</option>
          </select>
        </label>
      </div>
      <template v-for="item in slotLayout" :key="item.kind === 'slot' ? `s${item.n}` : `g${item.group.id}`">
        <SlotRow v-if="item.kind === 'slot'" :slot="item.n" />
        <GroupBox v-else :group="item.group" @edit="editing = $event" />
      </template>
      <form v-if="adding" class="addform" @submit.prevent="commitAdd" @keydown.esc.prevent="adding = false">
        <input ref="newInput" v-model="newName" maxlength="40" aria-label="Nome della nuova uscita" />
        <button type="submit" class="primary">Aggiungi</button>
        <button type="button" class="ghost" @click="adding = false">Annulla</button>
      </form>
      <div v-else class="addrow">
        <button class="addgroup" :class="{ primary: !state.outputs.length }" @click="startAdd">+ Aggiungi uscita</button>
        <button class="addgroup" @click="editing = null">+ Aggiungi gruppo</button>
      </div>

      <h2 class="mt">Partecipanti</h2>
      <label class="check">
        <input type="checkbox" :checked="state.settings.autoAdd" @change="toggleAutoAdd" />
        Aggiungi automaticamente al primo gruppo, in ordine di arrivo
      </label>
      <p v-if="state.settings.autoAdd && !state.groups.length" class="hint warnline">
        Nessun gruppo: crea un gruppo con le uscite da riempire, i partecipanti entrano nel primo.
      </p>
      <label class="field prio">Precedenza <small>(un nome per riga, il primo è il più importante)</small>
        <textarea v-model="priorityText" rows="4" placeholder="Mario Rossi&#10;Giulia" @blur="savePriority" />
      </label>
      <p class="hint">
        Chi è in elenco entra davanti a chi ha meno precedenza, e se le uscite sono piene l'ultimo va in attesa.
        "Mario" vale anche per "Mario Rossi". Gli spostamenti a mano restano; chi togli a mano non viene riaggiunto.
      </p>

      <h2 class="mt">Qualità inoltro</h2>
      <form class="settings" @submit.prevent="saveSettings">
        <label>Codec
          <select v-model="draft.codec">
            <option value="H264">H.264 (encoder hardware)</option>
            <option value="VP8">VP8</option>
            <option value="VP9">VP9</option>
            <option value="AV1">AV1</option>
          </select>
        </label>
        <label>Bitrate max (kbps)
          <input v-model="draft.maxBitrateKbps" type="number" min="1000" max="50000" step="500" />
        </label>
        <label>FPS max
          <input v-model="draft.maxFps" type="number" min="15" max="60" />
        </label>
        <label title="Nome del layout che OBSYard crea e riscrive in StreamYard per 'Ospiti a piena risoluzione'">Layout piena risoluzione
          <input v-model="draft.fullResLayout" maxlength="60" style="width: 160px" />
        </label>
        <button type="submit">Applica</button>
      </form>

      <h2 class="mt">OBS WebSocket <small>OBS → Strumenti → Impostazioni server WebSocket</small></h2>
      <form class="settings" @submit.prevent="saveObs">
        <label>Indirizzo
          <input v-model="obsDraft.url" placeholder="ws://127.0.0.1:4455" style="width: 180px" />
        </label>
        <label>Password
          <input v-model="obsDraft.password" type="password" autocomplete="off" />
        </label>
        <button type="submit">Connetti</button>
      </form>
      <p class="hint">
        API per Stream Deck: <code>http://127.0.0.1:{{ state.port }}/api/assign?slot=1&amp;video=Mario</code>
      </p>
    </section>
  </main>

  <GroupDialog v-if="editing !== undefined" :group="editing" @close="editing = undefined" />
</template>
