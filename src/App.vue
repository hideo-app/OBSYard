<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, reactive, ref, watch } from "vue";
import {
  state, init, videoTracks, audioTracks, slotLayout, setSettings, setObs, studio, addOutput, nextOutputName, setFullRes,
  selected, outputsOnScreen, sendTo, trackById, outputName, drag, applyPriority, obsPluginStatus, obsPluginInstall,
  type PluginStatus,
  type Settings, type GroupView, type ObsConfig, type TrackInfo,
} from "./store";
import { obs, startObs, obsWanted, createLtInputs } from "./obs";
import SourceCard from "./components/SourceCard.vue";
import SlotRow from "./components/SlotRow.vue";
import GroupBox from "./components/GroupBox.vue";
import GroupDialog from "./components/GroupDialog.vue";
import LowerThirdDialog from "./components/LowerThirdDialog.vue";
import ChoiceDialog from "./components/ChoiceDialog.vue";

const error = ref<string | null>(null);
const draft = ref<Settings>({ ...state.settings });
watch(() => state.settings, (s) => (draft.value = { ...s }), { deep: true });
const obsDraft = ref<ObsConfig>({ ...state.obs });
watch(() => state.obs, (o) => (obsDraft.value = { ...o }), { deep: true });

/** undefined = chiuso, null = nuovo gruppo */
const editing = ref<GroupView | null | undefined>(undefined);
/** sorgente di cui si modifica il sottopancia */
const ltEditing = ref<TrackInfo | null>(null);

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
const platformName = computed(() => (state.settings.platform === "discord" ? "Discord" : "StreamYard"));
const setPlatform = (e: Event) => {
  const select = e.target as HTMLSelectElement;
  const platform = select.value as "streamyard" | "discord";
  if (platform === state.settings.platform) return;
  if (!confirm(`Aprire ${platform === "discord" ? "Discord" : "StreamYard"} nella finestra studio? Le sorgenti attuali spariranno.`)) {
    select.value = state.settings.platform; // annullato: la tendina torna com'era
    return;
  }
  setSettings({ ...state.settings, platform });
};
const toggleDrive = () => setObs({ ...state.obs, drive: !state.obs.drive });
/** fps delle anteprime nella Regia */
const THUMB_FPS = [0.5, 1, 2, 5, 10, 15];
const setThumbFps = (e: Event) => setSettings({ ...state.settings, thumbFps: Number((e.target as HTMLSelectElement).value) });

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
/** salva l'elenco (se cambiato) e riordina comunque i gruppi, anche dopo spostamenti a mano */
const reorderNow = async () => {
  await savePriority();
  await applyPriority();
};

const fullResInfo = (): { cls: string; text: string; title?: string } | null => {
  const s = state.fullRes.status;
  if (state.settings.platform === "discord")
    return s === "on" ? { cls: "ok", text: "Qualità massima richiesta", title: "OBSYard chiede a Discord la qualità massima per ogni video. Il tetto resta quello di chi invia: camera 720p, Go Live secondo le sue impostazioni e Nitro" } : null;
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

// ---------- tally: quante uscite sono in programma ----------
const onAir = computed(() => Object.values(state.tally).filter((t) => t === "program").length);

// ---------- sottopancia ----------
const setLtMode = (e: Event) =>
  setSettings({ ...state.settings, lowerThirds: (e.target as HTMLSelectElement).value as Settings["lowerThirds"] });
const ltMissingCount = computed(() => Object.values(obs.ltMissing).reduce((n, l) => n + l.length, 0));
const ltBusy = ref(false);
const ltNote = ref("");
const createInObs = async () => {
  ltBusy.value = true;
  ltNote.value = "";
  try {
    const n = await createLtInputs();
    ltNote.value = `${n} fonti Testo create nella scena corrente: spostale e dai loro lo stile in OBS.`;
  } catch (e) {
    ltNote.value = String(e);
  } finally {
    ltBusy.value = false;
  }
};

// ---------- sorgente selezionata: Shift+1…9 la manda sull'uscita N, Esc deseleziona ----------
const selectedTrack = computed(() => trackById(selected.value));
watch(selectedTrack, (t) => !t && (selected.value = null)); // uscita di scena: niente selezione fantasma
const flash = ref("");
let flashTimer = 0;
const say = (text: string) => {
  flash.value = text;
  clearTimeout(flashTimer);
  flashTimer = window.setTimeout(() => (flash.value = ""), 2500);
};
const onKey = (e: KeyboardEvent) => {
  const el = e.target as HTMLElement;
  if (el.closest("input, textarea, select, .dialog")) return;
  if (e.key === "Escape") {
    if (drag.kind) drag.kind = null;
    else selected.value = null;
    return;
  }
  const m = /^(?:Digit|Numpad)([1-9])$/.exec(e.code);
  if (!m || !e.shiftKey || e.ctrlKey || e.altKey || e.metaKey) return;
  e.preventDefault();
  const t = selectedTrack.value;
  if (!t) return say("Seleziona prima una sorgente (clic sulla card), poi Shift+1…9");
  const slot = outputsOnScreen.value[Number(m[1]) - 1];
  if (slot == null) return say(`Non c'è un'uscita ${m[1]}`);
  void sendTo(slot, t.id);
  say(`${t.label} → ${outputName(slot)}`);
};
document.addEventListener("keydown", onKey);
onBeforeUnmount(() => document.removeEventListener("keydown", onKey));

// ---------- plugin OBS nativo ----------
const plugin = ref<PluginStatus | null>(null);
const pluginBusy = ref(false);
const pluginNote = ref("");
const refreshPlugin = async () => (plugin.value = await obsPluginStatus().catch(() => null));
onMounted(refreshPlugin);
const installPlugin = async () => {
  pluginBusy.value = true;
  pluginNote.value = "";
  try {
    plugin.value = await obsPluginInstall();
    pluginNote.value = "Installato: riavvia OBS per caricarlo.";
  } catch (e) {
    pluginNote.value = String(e);
  } finally {
    pluginBusy.value = false;
  }
};
const pluginLabel = computed(() => {
  const p = plugin.value;
  if (!p) return "";
  if (!p.available) return "non incluso";
  if (!p.installed) return "non installato";
  return p.current ? "installato" : "da aggiornare";
});
const nativeCount = computed(() => Object.keys(state.native).length);

// ---------- sezioni richiudibili (ricordate) ----------
const OPEN_KEY = "obsyard.sections";
const open = reactive<Record<string, boolean>>({ lt: false, people: false, quality: false, obs: false, plugin: false });
try {
  Object.assign(open, JSON.parse(localStorage.getItem(OPEN_KEY) ?? "{}"));
} catch { /* preferenze illeggibili: sezioni chiuse */ }
const toggleSection = (k: string, e: Event) => {
  open[k] = (e.target as HTMLDetailsElement).open;
  try {
    localStorage.setItem(OPEN_KEY, JSON.stringify(open));
  } catch { /* senza archivio le sezioni non vengono ricordate */ }
};
const ltModeLabel = computed(() => ({ off: "spenti", page: "sull'uscita", obs: "in fonti Testo di OBS" })[state.settings.lowerThirds]);
</script>

<template>
  <header class="top">
    <div class="brand"><span class="dot" :class="{ air: onAir }" />OBSYard</div>
    <select
      class="platform" :value="state.settings.platform" title="Piattaforma da cui prendere le sorgenti"
      @change="setPlatform($event)"
    >
      <option value="streamyard">StreamYard</option>
      <option value="discord">Discord</option>
    </select>
    <span class="pill" :class="state.publisherReady ? 'ok' : 'wait'">
      {{ state.publisherReady ? `${platformName} collegato` : `In attesa di ${platformName}…` }}
    </span>
    <span v-if="obsWanted()" class="pill" :class="obs.status === 'connected' ? 'ok' : 'wait'" :title="obs.error">{{ obsLabel() }}</span>
    <span v-if="onAir" class="pill air" title="Uscite in programma in OBS">ON AIR · {{ onAir }}</span>
    <span v-if="fullResInfo()" class="pill" :class="fullResInfo()!.cls" :title="fullResInfo()!.title">{{ fullResInfo()!.text }}</span>
    <div class="spacer" />
    <label class="switch" title="I gruppi cambiano scene ed elementi di OBS in base al numero di partecipanti">
      <input type="checkbox" :checked="state.obs.drive" @change="toggleDrive" /><span class="track" /> Pilota OBS
    </label>
    <label v-if="state.settings.platform === 'discord'" class="switch" title="Chiede a Discord la qualità massima per ogni video, anche quando il suo riquadro è piccolo. Chi invia resta al suo massimo: camera 720p, Go Live secondo le sue impostazioni e Nitro.">
      <input type="checkbox" :checked="state.fullRes.on" @change="setFullRes(!state.fullRes.on)" /><span class="track" /> Piena risoluzione
    </label>
    <label v-if="state.settings.platform === 'streamyard'" class="switch" title="Passa al layout personalizzato con tutti i riquadri a piena altezza: gli ospiti inviano alla massima risoluzione. Cambia anche il layout della diretta di StreamYard.">
      <input type="checkbox" :checked="state.fullRes.on" @change="setFullRes(!state.fullRes.on)" /><span class="track" /> Piena risoluzione
    </label>
    <label class="hsel" title="Quante volte al secondo si aggiornano le anteprime delle sorgenti nella Regia. Più alto = più fluide, più lavoro per il computer">
      Anteprime
      <select :value="state.settings.thumbFps" @change="setThumbFps">
        <option v-for="f in THUMB_FPS" :key="f" :value="f">{{ f < 1 ? `${f.toString().replace('.', ',')} fps` : `${f} fps` }}</option>
      </select>
    </label>
    <span class="vsep" />
    <button @click="studio('show')">Mostra {{ platformName }}</button>
    <button class="ghost" :title="`Ricarica ${platformName} nella finestra studio`" @click="studio('reload')">Ricarica</button>
  </header>

  <p v-if="error" class="err">{{ error }}</p>

  <main class="grid">
    <section class="panel">
      <h2>Sorgenti <small>{{ videoTracks.length }} video · {{ audioTracks.length }} audio</small></h2>
      <p v-if="!videoTracks.length" class="empty box">
        Nessuna sorgente. Entra nello studio StreamYard: ospiti, camera e schermi condivisi compariranno qui.
      </p>
      <p v-else class="tip" :class="{ sel: selectedTrack }" role="status">
        <template v-if="flash">{{ flash }}</template>
        <template v-else-if="selectedTrack">
          <strong>{{ selectedTrack.label }}</strong> selezionata: <kbd>Shift</kbd>+<kbd>1…9</kbd> la manda su un'uscita · <kbd>Esc</kbd> deseleziona
        </template>
        <template v-else>Trascina una sorgente su un'uscita, usa i numeri sulla card, oppure selezionala e premi <kbd>Shift</kbd>+<kbd>1…9</kbd>.</template>
      </p>
      <div class="cards">
        <SourceCard v-for="t in videoTracks" :key="t.id" :track="t" @lower-third="ltEditing = $event" />
      </div>
    </section>

    <section class="panel side">
      <div class="uhead">
        <h2>Uscite OBS <small>Fonte browser, 1920×1080</small></h2>
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

      <details class="sec" :open="open.lt" @toggle="toggleSection('lt', $event)">
        <summary>Sottopancia <small>{{ ltModeLabel }}</small></summary>
        <label class="field row">Dove vanno
          <select :value="state.settings.lowerThirds" @change="setLtMode">
            <option value="off">Spenti</option>
            <option value="page">Sull'uscita (disegnati dalla pagina)</option>
            <option value="obs">In fonti Testo di OBS</option>
          </select>
        </label>
        <template v-if="state.settings.lowerThirds === 'page'">
          <p class="hint">
            Ogni uscita disegna il nome di chi mostra, mentre ha video. Lo stile si cambia nel <em>CSS personalizzato</em> della Fonte browser:
            <code>.lower-third</code>, <code>.lower-third-name</code>, <code>.lower-third-title</code> e le variabili
            <code>--lt-bg</code>, <code>--lt-fg</code>, <code>--lt-accent</code>, <code>--lt-font</code>, <code>--lt-name-size</code>,
            <code>--lt-title-size</code>, <code>--lt-offset-x</code>, <code>--lt-offset-y</code>, <code>--lt-radius</code> (le stesse di guestroom.live).
          </p>
        </template>
        <template v-else-if="state.settings.lowerThirds === 'obs'">
          <p class="hint">
            OBSYard scrive nome e seconda riga in due fonti Testo per uscita, chiamate <code>&lt;uscita&gt; nome</code> e
            <code>&lt;uscita&gt; titolo</code> (es. <code>Cam 1 nome</code>), e le svuota quando l'uscita si svuota. Le pagine non disegnano nulla.
          </p>
          <p v-if="obs.status !== 'connected'" class="hint warnline">{{ obsLabel() }}: controlla indirizzo e password in OBS WebSocket.</p>
          <p v-else-if="ltMissingCount" class="hint warnline">
            In OBS mancano {{ ltMissingCount }} fonti Testo.
            <button class="mini-btn" :disabled="ltBusy" @click="createInObs">Crea in OBS</button>
          </p>
          <p v-else class="hint okline">Tutte le fonti Testo ci sono.</p>
          <p v-if="ltNote" class="hint">{{ ltNote }}</p>
        </template>
        <p v-if="state.settings.lowerThirds !== 'off'" class="hint">
          Nome e seconda riga di una persona: pulsante <strong>Aa</strong> sulla sua card. Per escludere un'uscita: menu ⋮ → <em>Sottopancia su questa uscita</em>.
        </p>
      </details>

      <details class="sec" :open="open.people" @toggle="toggleSection('people', $event)">
        <summary>Partecipanti <small>{{ state.settings.autoAdd ? "aggiunta automatica" : "aggiunta a mano" }}{{ state.settings.priority.length ? ` · ${state.settings.priority.length} con precedenza` : "" }}</small></summary>
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
        <button
          type="button" class="mini-btn" :disabled="!parseNames(priorityText).length || !state.groups.length"
          title="Rimette i membri dei gruppi nell'ordine della precedenza, annullando gli spostamenti fatti a mano"
          @click="reorderNow"
        >Riordina ora</button>
        <p class="hint">
          Chi è in elenco entra davanti a chi ha meno precedenza, e se le uscite sono piene l'ultimo va in attesa.
          "Mario" vale anche per "Mario Rossi". Cambiando l'elenco i gruppi si riordinano una volta; dopo, gli spostamenti a mano restano
          finché non premi <em>Riordina ora</em>. Chi togli a mano non viene riaggiunto.
        </p>
      </details>

      <details class="sec" :open="open.quality" @toggle="toggleSection('quality', $event)">
        <summary>Qualità inoltro <small>{{ state.settings.codec }} · {{ state.settings.maxBitrateKbps / 1000 }} Mbps · {{ state.settings.maxFps }} fps</small></summary>
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
          <label title="Nome del layout che OBSYard crea e riscrive in StreamYard per 'Piena risoluzione'">Layout piena risoluzione
            <input v-model="draft.fullResLayout" maxlength="60" style="width: 160px" />
          </label>
          <button type="submit">Applica</button>
        </form>
      </details>

      <details class="sec" :open="open.plugin" @toggle="toggleSection('plugin', $event); refreshPlugin()">
        <summary>Plugin OBS <small>{{ pluginLabel }}{{ nativeCount ? ` · ${nativeCount} uscite lette` : "" }}</small></summary>
        <p class="hint first">
          Con il plugin, in OBS si aggiunge una sorgente <strong>OBSYard</strong> e si sceglie l'uscita dalla tendina:
          immagine e audio arrivano come OBSYard li decodifica, <em>senza ricodifica</em>, con meno ritardo e meno lavoro per la scheda video.
          Le Fonti browser continuano a funzionare come prima.
        </p>
        <p v-if="plugin && !plugin.obsFound" class="hint warnline">OBS non trovato in Programmi: il plugin si installa lo stesso, per quando ci sarà.</p>
        <p v-if="plugin && plugin.available" class="hint">
          <button class="mini-btn" :disabled="pluginBusy || plugin.current" @click="installPlugin">
            {{ plugin.current ? "Installato" : plugin.installed ? "Aggiorna il plugin" : "Installa il plugin" }}
          </button>
          <span v-if="pluginNote"> {{ pluginNote }}</span>
        </p>
        <p v-else-if="plugin" class="hint warnline">Questa versione di OBSYard non include il plugin.</p>
        <p class="hint">
          Nella sorgente: <em>Risoluzione</em> fissa (1920×1080 di serie, l'immagine sta dentro con bande trasparenti),
          <em>Ricevi l'audio</em> attivabile. I sottopancia, con il plugin, vanno nelle fonti Testo di OBS.
        </p>
      </details>

      <details class="sec" :open="open.obs" @toggle="toggleSection('obs', $event)">
        <summary>OBS WebSocket <small>{{ obsWanted() ? obsLabel() : "non in uso" }}</small></summary>
        <p class="hint first">In OBS: Strumenti → Impostazioni server WebSocket. Serve per <em>Pilota OBS</em> e per i sottopancia in fonti Testo.</p>
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
      </details>
    </section>
  </main>

  <GroupDialog v-if="editing !== undefined" :group="editing" @close="editing = undefined" />
  <LowerThirdDialog v-if="ltEditing" :track="ltEditing" @close="ltEditing = null" />
  <ChoiceDialog />
</template>
