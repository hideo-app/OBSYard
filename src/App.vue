<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, reactive, ref, watch } from "vue";
import {
  state, init, videoTracks, audioTracks, slotLayout, setSettings, setObs, studio, addOutput, nextOutputName, setFullRes,
  selected, outputsOnScreen, sendTo, trackById, outputName, drag, applyPriority, obsPluginStatus, obsPluginInstall,
  type PluginStatus,
  type Settings, type GroupView, type ObsConfig, type TrackInfo,
} from "./store";
import { obs, startObs, obsWanted, createLtInputs } from "./obs";
import { t } from "./i18n";
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
  if (!confirm(t("top.switchPlatform", { p: platform === "discord" ? "Discord" : "StreamYard" }))) {
    select.value = state.settings.platform; // annullato: la tendina torna com'era
    return;
  }
  setSettings({ ...state.settings, platform });
};
const toggleDrive = () => setObs({ ...state.obs, drive: !state.obs.drive });
/** fps delle anteprime nella Regia */
const THUMB_FPS = [0.5, 1, 2, 5, 10, 15];
const setThumbFps = (e: Event) => setSettings({ ...state.settings, thumbFps: Number((e.target as HTMLSelectElement).value) });
const fpsLabel = (f: number) => `${f < 1 ? f.toLocaleString(navigator.language) : f} fps`;

// partecipanti: aggiunta automatica al primo gruppo (anche solo audio) + precedenza (un nome per riga)
const toggleAutoAdd = () => setSettings({ ...state.settings, autoAdd: !state.settings.autoAdd });
const toggleAutoAddAudio = () => setSettings({ ...state.settings, autoAddAudio: !state.settings.autoAddAudio });
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
const peopleSummary = computed(() => {
  const s = state.settings;
  const mode = !s.autoAdd ? t("ppl.manual") : s.autoAddAudio ? t("ppl.autoAudio") : t("ppl.auto");
  return mode + (s.priority.length ? t("ppl.withPrio", { n: s.priority.length }) : "");
});

const fullResInfo = (): { cls: string; text: string; title?: string } | null => {
  const s = state.fullRes.status;
  if (state.settings.platform === "discord")
    return s === "on" ? { cls: "ok", text: t("fr.discordOn"), title: t("fr.discordOnTitle") } : null;
  const n = state.fullRes.layout;
  if (s.startsWith("error:")) return { cls: "bad", text: t("fr.error"), title: s.slice(6) };
  return (
    ({
      off: null,
      "": { cls: "wait", text: t("fr.busy") },
      on: { cls: "ok", text: t("fr.on", { n }), title: t("fr.onTitle") },
      created: { cls: "wait", text: t("fr.created", { n }), title: t("fr.createdTitle") },
      updated: { cls: "wait", text: t("fr.updated", { n }), title: t("fr.updatedTitle") },
      missing: { cls: "bad", text: t("fr.missing", { n }), title: t("fr.missingTitle") },
      manual: { cls: "wait", text: t("fr.manual"), title: t("fr.manualTitle") },
    } as Record<string, { cls: string; text: string; title?: string } | null>)[s] ?? null
  );
};

const obsLabel = () =>
  ({ idle: t("obs.idle"), connecting: t("obs.connecting"), connected: t("obs.connected"), auth: t("obs.auth"), error: t("obs.error") })[obs.status];

// ---------- tally: quante uscite sono in programma ----------
const onAir = computed(() => Object.values(state.tally).filter((x) => x === "program").length);

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
    ltNote.value = t("lt.created", { n });
  } catch (e) {
    ltNote.value = String(e);
  } finally {
    ltBusy.value = false;
  }
};
const LT_HOOKS = [".lower-third", ".lower-third-name", ".lower-third-title", "--lt-bg", "--lt-fg", "--lt-accent", "--lt-font", "--lt-name-size", "--lt-title-size", "--lt-offset-x", "--lt-offset-y", "--lt-radius"];

// ---------- sorgente selezionata: Shift+1…9 la manda sull'uscita N, Esc deseleziona ----------
const selectedTrack = computed(() => trackById(selected.value));
watch(selectedTrack, (x) => !x && (selected.value = null)); // uscita di scena: niente selezione fantasma
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
  const tr = selectedTrack.value;
  if (!tr) return say(t("src.selectFirst"));
  const slot = outputsOnScreen.value[Number(m[1]) - 1];
  if (slot == null) return say(t("src.noOutput", { n: m[1] }));
  void sendTo(slot, tr.id);
  say(`${tr.label} → ${outputName(slot)}`);
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
    pluginNote.value = t("pl.done");
  } catch (e) {
    pluginNote.value = String(e);
  } finally {
    pluginBusy.value = false;
  }
};
const pluginLabel = computed(() => {
  const p = plugin.value;
  if (!p) return "";
  if (!p.available) return t("pl.notIncluded");
  if (!p.installed) return t("pl.notInstalled");
  return p.current ? t("pl.installed") : t("pl.outdated");
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
const ltModeLabel = computed(() => ({ off: t("lt.off"), page: t("lt.page"), obs: t("lt.obs") })[state.settings.lowerThirds]);
</script>

<template>
  <header class="top">
    <div class="brand"><span class="dot" :class="{ air: onAir }" />OBSYard</div>
    <select class="platform" :value="state.settings.platform" :title="t('top.platform')" @change="setPlatform($event)">
      <option value="streamyard">StreamYard</option>
      <option value="discord">Discord</option>
    </select>
    <span class="pill" :class="state.publisherReady ? 'ok' : 'wait'">
      {{ state.publisherReady ? t("top.connected", { p: platformName }) : t("top.waiting", { p: platformName }) }}
    </span>
    <span v-if="obsWanted()" class="pill" :class="obs.status === 'connected' ? 'ok' : 'wait'" :title="obs.error">{{ obsLabel() }}</span>
    <span v-if="onAir" class="pill air" :title="t('top.onAirTitle')">ON AIR · {{ onAir }}</span>
    <span v-if="fullResInfo()" class="pill" :class="fullResInfo()!.cls" :title="fullResInfo()!.title">{{ fullResInfo()!.text }}</span>
    <div class="spacer" />
    <label class="switch" :title="t('top.driveTitle')">
      <input type="checkbox" :checked="state.obs.drive" @change="toggleDrive" /><span class="track" /> {{ t("top.drive") }}
    </label>
    <label class="switch" :title="state.settings.platform === 'discord' ? t('top.fullResDiscord') : t('top.fullResSy')">
      <input type="checkbox" :checked="state.fullRes.on" @change="setFullRes(!state.fullRes.on)" /><span class="track" /> {{ t("top.fullRes") }}
    </label>
    <label class="hsel" :title="t('top.previewsTitle')">
      {{ t("top.previews") }}
      <select :value="state.settings.thumbFps" @change="setThumbFps">
        <option v-for="f in THUMB_FPS" :key="f" :value="f">{{ fpsLabel(f) }}</option>
      </select>
    </label>
    <span class="vsep" />
    <button @click="studio('show')">{{ t("top.show", { p: platformName }) }}</button>
    <button class="ghost" :title="t('top.reloadTitle', { p: platformName })" @click="studio('reload')">{{ t("top.reload") }}</button>
  </header>

  <p v-if="error" class="err">{{ error }}</p>

  <main class="grid">
    <section class="panel">
      <h2>{{ t("src.title") }} <small>{{ t("src.count", { v: videoTracks.length, a: audioTracks.length }) }}</small></h2>
      <p v-if="!videoTracks.length" class="empty box">{{ t("src.empty", { p: platformName }) }}</p>
      <p v-else class="tip" :class="{ sel: selectedTrack }" role="status">
        <template v-if="flash">{{ flash }}</template>
        <template v-else-if="selectedTrack">
          <strong>{{ selectedTrack.label }}</strong> {{ t("src.selected") }} <kbd>Shift</kbd>+<kbd>1…9</kbd> {{ t("src.sendsTo") }} · <kbd>Esc</kbd> {{ t("src.deselect") }}
        </template>
        <template v-else>{{ t("src.tip") }} <kbd>Shift</kbd>+<kbd>1…9</kbd>.</template>
      </p>
      <div class="cards">
        <SourceCard v-for="tr in videoTracks" :key="tr.id" :track="tr" @lower-third="ltEditing = $event" />
      </div>
    </section>

    <section class="panel side">
      <div class="uhead">
        <h2>{{ t("out.title") }} <small>{{ t("out.subtitle") }}</small></h2>
        <label class="bgpick" :title="t('out.emptyBgTitle')">
          {{ t("out.emptyBg") }}
          <select :value="state.settings.emptyBg" @change="setSettings({ ...state.settings, emptyBg: ($event.target as HTMLSelectElement).value as 'transparent' | 'black' })">
            <option value="transparent">{{ t("out.transparent") }}</option>
            <option value="black">{{ t("out.black") }}</option>
          </select>
        </label>
      </div>
      <template v-for="item in slotLayout" :key="item.kind === 'slot' ? `s${item.n}` : `g${item.group.id}`">
        <SlotRow v-if="item.kind === 'slot'" :slot="item.n" />
        <GroupBox v-else :group="item.group" @edit="editing = $event" />
      </template>
      <form v-if="adding" class="addform" @submit.prevent="commitAdd" @keydown.esc.prevent="adding = false">
        <input ref="newInput" v-model="newName" maxlength="40" :aria-label="t('out.newName')" />
        <button type="submit" class="primary">{{ t("out.add") }}</button>
        <button type="button" class="ghost" @click="adding = false">{{ t("out.cancel") }}</button>
      </form>
      <div v-else class="addrow">
        <button class="addgroup" :class="{ primary: !state.outputs.length }" @click="startAdd">{{ t("out.addOutput") }}</button>
        <button class="addgroup" @click="editing = null">{{ t("out.addGroup") }}</button>
      </div>

      <details class="sec" :open="open.lt" @toggle="toggleSection('lt', $event)">
        <summary>{{ t("lt.title") }} <small>{{ ltModeLabel }}</small></summary>
        <label class="field row">{{ t("lt.where") }}
          <select :value="state.settings.lowerThirds" @change="setLtMode">
            <option value="off">{{ t("lt.optOff") }}</option>
            <option value="page">{{ t("lt.optPage") }}</option>
            <option value="obs">{{ t("lt.optObs") }}</option>
          </select>
        </label>
        <p v-if="state.settings.lowerThirds === 'page'" class="hint">
          {{ t("lt.pageHint") }}
          <template v-for="(h, i) in LT_HOOKS" :key="h"><code>{{ h }}</code>{{ i < LT_HOOKS.length - 1 ? " " : "." }}</template>
        </p>
        <template v-else-if="state.settings.lowerThirds === 'obs'">
          <p class="hint">{{ t("lt.obsHint") }}</p>
          <p v-if="obs.status !== 'connected'" class="hint warnline">{{ t("lt.obsCheck", { s: obsLabel() }) }}</p>
          <p v-else-if="ltMissingCount" class="hint warnline">
            {{ t("lt.missing", { n: ltMissingCount }) }}
            <button class="mini-btn" :disabled="ltBusy" @click="createInObs">{{ t("lt.create") }}</button>
          </p>
          <p v-else class="hint okline">{{ t("lt.allThere") }}</p>
          <p v-if="ltNote" class="hint">{{ ltNote }}</p>
        </template>
        <p v-if="state.settings.lowerThirds !== 'off'" class="hint">{{ t("lt.howTo") }}</p>
      </details>

      <details class="sec" :open="open.people" @toggle="toggleSection('people', $event)">
        <summary>{{ t("ppl.title") }} <small>{{ peopleSummary }}</small></summary>
        <label class="check">
          <input type="checkbox" :checked="state.settings.autoAdd" @change="toggleAutoAdd" />
          {{ t("ppl.autoAdd") }}
        </label>
        <label class="check sub" :class="{ dim: !state.settings.autoAdd }">
          <input type="checkbox" :checked="state.settings.autoAddAudio" :disabled="!state.settings.autoAdd" @change="toggleAutoAddAudio" />
          {{ t("ppl.autoAddAudio") }}
        </label>
        <p v-if="state.settings.autoAdd && !state.groups.length" class="hint warnline">{{ t("ppl.noGroup") }}</p>
        <label class="field prio">{{ t("ppl.priority") }} <small>{{ t("ppl.priorityNote") }}</small>
          <textarea v-model="priorityText" rows="4" placeholder="Mario Rossi&#10;Giulia" @blur="savePriority" />
        </label>
        <button
          type="button" class="mini-btn" :disabled="!parseNames(priorityText).length || !state.groups.length"
          :title="t('ppl.reorderTitle')" @click="reorderNow"
        >{{ t("ppl.reorder") }}</button>
        <p class="hint">{{ t("ppl.hint") }}</p>
      </details>

      <details class="sec" :open="open.quality" @toggle="toggleSection('quality', $event)">
        <summary>{{ t("q.title") }} <small>{{ state.settings.codec }} · {{ state.settings.maxBitrateKbps / 1000 }} Mbps · {{ state.settings.maxFps }} fps</small></summary>
        <form class="settings" @submit.prevent="saveSettings">
          <label>{{ t("q.codec") }}
            <select v-model="draft.codec">
              <option value="H264">{{ t("q.h264") }}</option>
              <option value="VP8">VP8</option>
              <option value="VP9">VP9</option>
              <option value="AV1">AV1</option>
            </select>
          </label>
          <label>{{ t("q.bitrate") }}
            <input v-model="draft.maxBitrateKbps" type="number" min="1000" max="50000" step="500" />
          </label>
          <label>{{ t("q.fps") }}
            <input v-model="draft.maxFps" type="number" min="15" max="60" />
          </label>
          <label :title="t('q.layoutTitle')">{{ t("q.layout") }}
            <input v-model="draft.fullResLayout" maxlength="60" style="width: 160px" />
          </label>
          <button type="submit">{{ t("q.apply") }}</button>
        </form>
      </details>

      <details class="sec" :open="open.plugin" @toggle="toggleSection('plugin', $event); refreshPlugin()">
        <summary>{{ t("pl.title") }} <small>{{ pluginLabel }}{{ nativeCount ? t("pl.reading", { n: nativeCount }) : "" }}</small></summary>
        <p class="hint first">{{ t("pl.intro") }}</p>
        <p v-if="plugin && !plugin.obsFound" class="hint warnline">{{ t("pl.noObs") }}</p>
        <p v-if="plugin && plugin.available" class="hint">
          <button class="mini-btn" :disabled="pluginBusy || plugin.current" @click="installPlugin">
            {{ plugin.current ? t("pl.isInstalled") : plugin.installed ? t("pl.update") : t("pl.install") }}
          </button>
          <span v-if="pluginNote"> {{ pluginNote }}</span>
        </p>
        <p v-else-if="plugin" class="hint warnline">{{ t("pl.notInThis") }}</p>
        <p class="hint">{{ t("pl.sourceHint") }}</p>
      </details>

      <details class="sec" :open="open.obs" @toggle="toggleSection('obs', $event)">
        <summary>{{ t("ws.title") }} <small>{{ obsWanted() ? obsLabel() : t("ws.unused") }}</small></summary>
        <p class="hint first">{{ t("ws.hint") }}</p>
        <form class="settings" @submit.prevent="saveObs">
          <label>{{ t("ws.address") }}
            <input v-model="obsDraft.url" placeholder="ws://127.0.0.1:4455" style="width: 180px" />
          </label>
          <label>{{ t("ws.password") }}
            <input v-model="obsDraft.password" type="password" autocomplete="off" />
          </label>
          <button type="submit">{{ t("ws.connect") }}</button>
        </form>
        <p class="hint">
          {{ t("ws.api") }} <code>http://127.0.0.1:{{ state.port }}/api/assign?slot=1&amp;video=Mario</code>
        </p>
      </details>
    </section>
  </main>

  <GroupDialog v-if="editing !== undefined" :group="editing" @close="editing = undefined" />
  <LowerThirdDialog v-if="ltEditing" :track="ltEditing" @close="ltEditing = null" />
  <ChoiceDialog />
</template>
