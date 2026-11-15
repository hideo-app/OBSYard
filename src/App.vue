<script setup lang="ts">
import { onMounted, ref, watch } from "vue";
import {
  state, init, videoTracks, audioTracks, slotLayout, setSettings, setObs, studio,
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
const toggleDrive = () => setObs({ ...state.obs, drive: !state.obs.drive });

const obsLabel = () =>
  ({ idle: "OBS: non configurato", connecting: "OBS: connessione…", connected: "OBS connesso", auth: "OBS: password errata", error: "OBS non raggiungibile" })[obs.status];
</script>

<template>
  <header class="top">
    <div class="brand"><span class="dot" />OBSYard</div>
    <span class="pill" :class="state.publisherReady ? 'ok' : 'wait'">
      {{ state.publisherReady ? "StreamYard collegato" : "In attesa di StreamYard…" }}
    </span>
    <span class="pill" :class="obs.status === 'connected' ? 'ok' : 'wait'" :title="obs.error">{{ obsLabel() }}</span>
    <label class="drive" title="I gruppi cambiano scene ed elementi di OBS in base al numero di partecipanti">
      <input type="checkbox" :checked="state.obs.drive" @change="toggleDrive" /> Pilota OBS
    </label>
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
      <h2>Slot OBS <small>Fonte browser, consigliato 1920×1080</small></h2>
      <template v-for="item in slotLayout" :key="item.kind === 'slot' ? `s${item.n}` : `g${item.group.id}`">
        <SlotRow v-if="item.kind === 'slot'" :slot="item.n" />
        <GroupBox v-else :group="item.group" @edit="editing = $event" />
      </template>
      <button class="addgroup" @click="editing = null">+ Aggiungi gruppo</button>

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
