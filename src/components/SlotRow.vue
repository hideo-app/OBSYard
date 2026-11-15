<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watch } from "vue";
import {
  state, thumbs, assign, videoTracks, audioTracks, trackById, slotUrl, slotNameUrl, outputById,
  updateOutput, moveOutput, deleteOutput, groupOfSlot, outputBg, tallyOf, drag, sendTo, swapOutputs,
  groupMove, outputStats, outputName, outputsOnScreen, alsoOn, type OutputStats,
} from "../store";
import { obs } from "../obs";
import { t } from "../i18n";

const props = defineProps<{ slot: string | number; fillIndex?: number }>();

const id = computed(() => Number(props.slot));
const out = computed(() => outputById(props.slot));
const a = computed(() => state.slots[props.slot] ?? { video: null, audio: "auto" });
const video = computed(() => trackById(a.value.video));
const viewers = computed(() => state.viewers[props.slot] ?? 0);
/** la sorgente "OBSYard" del plugin OBS legge questa uscita (senza ricodifica) */
const native = computed(() => !!state.native[String(props.slot)]);
const group = computed(() => groupOfSlot(props.slot));
const inGroup = computed(() => props.fillIndex != null);
const tally = computed(() => tallyOf(props.slot));
/** numero dell'uscita a schermo: quello dei pulsanti sulle card e di Shift+N */
const screenIndex = computed(() => outputsOnScreen.value.indexOf(id.value) + 1);
/** uscita riempita dalla prova di riempimento del gruppo */
const inTest = computed(() => !!group.value?.testFill?.slots.includes(id.value));
const lt = computed(() => state.lowerThirds[String(props.slot)]);
/** altre uscite che mostrano lo stesso video: avviso "duplicato" */
const dupOf = computed(() => (a.value.video ? alsoOn(a.value.video, props.slot) : []));
const dupTitle = computed(() => {
  const where = dupOf.value.map((o) => outputName(o)).join(", ");
  const grouped = state.dups.includes(id.value) ? t("row.dupGroup") : "";
  return t("row.dupTitle", { w: where }) + grouped;
});
const ltMissing = computed(() => obs.ltMissing[id.value] ?? []);
const copied = ref(false);

const setVideo = (e: Event) => {
  const v = (e.target as HTMLSelectElement).value || null;
  assign(props.slot, { ...a.value, video: v });
};
const setAudio = (e: Event) => {
  const v = (e.target as HTMLSelectElement).value || null;
  assign(props.slot, { ...a.value, audio: v });
};
const copy = async (byName = false) => {
  menu.value = false;
  await navigator.clipboard.writeText(byName ? slotNameUrl(props.slot) : slotUrl(props.slot));
  copied.value = true;
  setTimeout(() => (copied.value = false), 1200);
};

// ---------- menu ⋮ ----------
const menu = ref(false);
const renaming = ref(false);
const draftName = ref("");
const nameInput = ref<HTMLInputElement>();
const closeMenu = (e: MouseEvent) => {
  if (!(e.target as HTMLElement).closest?.(".omenu-wrap")) menu.value = false;
};
document.addEventListener("click", closeMenu);
onBeforeUnmount(() => document.removeEventListener("click", closeMenu));

const startRename = async () => {
  menu.value = false;
  draftName.value = out.value?.name ?? "";
  renaming.value = true;
  await nextTick();
  nameInput.value?.select();
};
const commitRename = async () => {
  if (!renaming.value) return;
  renaming.value = false;
  const n = draftName.value.trim();
  if (n && n !== out.value?.name) await updateOutput(id.value, { name: n });
};
const setBg = (bg: "default" | "transparent" | "black") => {
  menu.value = false;
  updateOutput(id.value, { bg });
};
const bgChoice = computed(() => out.value?.bg ?? "default");
const toggleLt = () => {
  menu.value = false;
  updateOutput(id.value, { lt: !out.value?.lt });
};
const move = (d: -1 | 1) => moveOutput(id.value, d);
/** sposta di un posto nel gruppo la persona che è su questa uscita */
const movePerson = (d: -1 | 1) => {
  menu.value = false;
  if (group.value && a.value.video) groupMove(group.value.id, a.value.video, d);
};
/** Alt+↑/↓: in un gruppo sposta la persona, altrimenti l'uscita nell'elenco */
const altArrow = (d: -1 | 1) => (inGroup.value ? movePerson(d) : move(d));
const remove = () => {
  menu.value = false;
  const g = group.value;
  const extra = g ? t("row.deleteGroup", { g: g.name }) : "";
  if (confirm(t("row.deleteConfirm", { n: out.value?.name ?? "" }) + extra))
    deleteOutput(id.value);
};

// ---------- trascinamento: sorgente → uscita, uscita → uscita ----------
const over = ref(false);
const canDrop = computed(() => drag.kind === "track" || (drag.kind === "output" && drag.id !== String(props.slot)));
/** cosa farà il rilascio, mostrato sull'uscita sotto il puntatore */
const dropHint = computed(() => {
  if (drag.kind === "track") {
    if (a.value.video === drag.id) return t("drop.here");
    if (group.value?.members.some((m) => m.track === drag.id)) return t("drop.swapPlace");
    return inGroup.value ? t("drop.insert") : video.value ? t("drop.replace") : t("drop.send");
  }
  if (drag.kind === "output") {
    const from = groupOfSlot(drag.id);
    if (!from && !group.value) return t("drop.swap");
    if (from && from.id === group.value?.id) return t("drop.swapPlace");
    return t("drop.sendFrom", { o: outputName(drag.id) });
  }
  return "";
});
const onDragOver = (e: DragEvent) => {
  if (!canDrop.value) return;
  e.preventDefault();
  e.dataTransfer!.dropEffect = "move";
  over.value = true;
};
const onDragLeave = (e: DragEvent) => {
  if (!(e.currentTarget as HTMLElement).contains(e.relatedTarget as Node | null)) over.value = false;
};
const onDrop = (e: DragEvent) => {
  e.preventDefault();
  over.value = false;
  const { kind, id: from } = drag;
  drag.kind = null;
  if (kind === "track" && a.value.video !== from) sendTo(props.slot, from);
  else if (kind === "output") swapOutputs(Number(from), id.value);
};
const onDragStart = (e: DragEvent) => {
  e.dataTransfer!.setData("text/plain", out.value?.name ?? "");
  e.dataTransfer!.effectAllowed = "move";
  drag.kind = "output";
  drag.id = String(props.slot);
};

// ---------- dettagli della connessione ----------
const showStats = ref(false);
const stats = ref<OutputStats[]>([]);
let statsTimer = 0;
const pollStats = async () => (stats.value = await outputStats(id.value).catch(() => []));
watch(showStats, (on) => {
  clearInterval(statsTimer);
  stats.value = [];
  if (!on) return;
  void pollStats();
  statsTimer = window.setInterval(pollStats, 1000);
});
onBeforeUnmount(() => clearInterval(statsTimer));
const openStats = () => {
  menu.value = false;
  showStats.value = !showStats.value;
};
const stateLabel = (s: string) =>
  ({ connected: t("st.connected"), connecting: t("st.connecting"), new: t("st.connecting"), failed: t("st.failed"), disconnected: t("st.disconnected"), closed: t("st.closed") })[s] ?? s;
const mbps = (k?: number) => (k == null ? "…" : (k / 1000).toFixed(1));
</script>

<template>
  <div
    class="slot" :class="{ live: video, grouped: inGroup, program: tally === 'program', preview: tally === 'preview', over: over && canDrop, droppable: canDrop }"
    tabindex="0"
    @keydown.alt.up.self.prevent="altArrow(-1)" @keydown.alt.down.self.prevent="altArrow(1)"
    @dragover="onDragOver" @dragleave="onDragLeave" @drop="onDrop"
  >
    <div class="num" :title="(inGroup ? t('row.place', { n: fillIndex ?? 0 }) : '') + (screenIndex <= 9 ? t('row.shift', { n: screenIndex }) : t('row.output', { n: screenIndex }))">
      {{ screenIndex }}
    </div>
    <div
      class="mini" :class="{ black: outputBg(slot) === 'black', grab: !!a.video }" :draggable="!!a.video"
      :title="a.video ? t('row.dragSwap') : undefined"
      @dragstart="onDragStart" @dragend="drag.kind = null"
    >
      <img v-if="video && thumbs[video.id]" :src="thumbs[video.id]" alt="" draggable="false" />
    </div>
    <div class="selects">
      <div class="oname">
        <input
          v-if="renaming" ref="nameInput" v-model="draftName" class="rename" maxlength="40"
          @keydown.enter.prevent="commitRename" @keydown.esc.prevent="renaming = false" @blur="commitRename"
        />
        <strong v-else :title="t('row.renameTitle', { id: slot })" @dblclick="startRename">{{ out?.name ?? `Slot ${slot}` }}</strong>
        <span v-if="tally === 'program'" class="tag air" :title="t('row.onAirTitle')">ON AIR</span>
        <span v-else-if="tally === 'preview'" class="tag pre" :title="t('row.previewTitle')">{{ t("card.preview") }}</span>
        <span v-if="dupOf.length" class="tag dup" :title="dupTitle">{{ t("row.dup") }}</span>
        <span v-if="inTest" class="tag test" :title="t('row.testTitle')">{{ t("row.test") }}</span>
        <span v-if="out?.bg" class="tag" :title="t('row.bgTitle')">{{ out.bg === "black" ? t("row.black") : t("row.transparent") }}</span>
        <span v-if="ltMissing.length" class="tag bad" :title="t('row.textMissingTitle', { l: ltMissing.join('\n') })">{{ t("row.textMissing") }}</span>
      </div>
      <select :value="a.video ?? ''" :aria-label="t('row.videoSource')" @change="setVideo">
        <option value="">{{ inGroup ? t("row.emptyGroup") : t("row.empty") }}</option>
        <option v-if="a.video && !video" :value="a.video">{{ t("row.gone") }}</option>
        <option v-for="tr in videoTracks" :key="tr.id" :value="tr.id">{{ tr.audioOnly ? `${tr.label} (${t("card.audioOnly").toLowerCase()})` : tr.label }}</option>
      </select>
      <select v-if="!inGroup" :value="a.audio ?? ''" :aria-label="t('row.audioSource')" @change="setAudio">
        <option value="auto">{{ t("row.audioAuto") }}</option>
        <option value="">{{ t("row.audioNone") }}</option>
        <option v-for="tr in audioTracks" :key="tr.id" :value="tr.id">{{ t("row.audioOf", { l: tr.label }) }}</option>
      </select>
      <div v-if="lt" class="ltline" :title="state.settings.lowerThirds === 'obs' ? t('row.ltObs') : t('row.ltPage')">
        <span class="ltmark">Aa</span> {{ lt.name }}<template v-if="lt.title"> · {{ lt.title }}</template>
      </div>
    </div>
    <button class="url" :title="t('row.copyTitle', { u: slotUrl(slot), n: slotNameUrl(slot) })" @click="copy()">{{ copied ? t("row.copied") : t("row.copyUrl") }}</button>
    <span v-if="native" class="tag plug" :title="t('row.pluginTitle')">{{ t("row.plugin") }}</span>
    <button
      class="obs" :class="{ on: viewers, open: showStats }" :aria-expanded="showStats"
      :title="viewers ? t('row.viewersTitle', { n: viewers }) : t('row.noViewers')"
      @click="openStats"
    ><span class="led" />OBS {{ viewers }}</button>
    <div class="omenu-wrap">
      <button class="icon" :title="t('row.more')" aria-haspopup="menu" :aria-expanded="menu" @click="menu = !menu">⋮</button>
      <div v-if="menu" class="omenu" role="menu">
        <button role="menuitem" @click="startRename">{{ t("row.rename") }}</button>
        <button role="menuitem" :title="slotNameUrl(slot)" @click="copy(true)">{{ t("row.copyByName") }}</button>
        <template v-if="inGroup && a.video">
          <button role="menuitem" @click="movePerson(-1)">{{ t("row.personUp") }} <kbd>Alt+↑</kbd></button>
          <button role="menuitem" @click="movePerson(1)">{{ t("row.personDown") }} <kbd>Alt+↓</kbd></button>
        </template>
        <button role="menuitem" @click="move(-1)">{{ t("row.outUp") }} <kbd v-if="!inGroup">Alt+↑</kbd></button>
        <button role="menuitem" @click="move(1)">{{ t("row.outDown") }} <kbd v-if="!inGroup">Alt+↓</kbd></button>
        <button role="menuitem" @click="openStats">{{ showStats ? t("row.hideStats") : t("row.showStats") }}</button>
        <button v-if="state.settings.lowerThirds !== 'off'" role="menuitemcheckbox" :aria-checked="!!out?.lt" @click="toggleLt">
          {{ out?.lt ? "✓ " : "" }}{{ t("row.ltHere") }}
        </button>
        <div class="msep">{{ t("out.emptyBg") }}</div>
        <button role="menuitemradio" :aria-checked="bgChoice === 'default'" @click="setBg('default')">
          {{ bgChoice === "default" ? "✓ " : "" }}{{ t("row.asSetting", { b: state.settings.emptyBg === "black" ? t("row.black") : t("row.transparent") }) }}
        </button>
        <button role="menuitemradio" :aria-checked="bgChoice === 'transparent'" @click="setBg('transparent')">
          {{ bgChoice === "transparent" ? "✓ " : "" }}{{ t("out.transparent") }}
        </button>
        <button role="menuitemradio" :aria-checked="bgChoice === 'black'" @click="setBg('black')">
          {{ bgChoice === "black" ? "✓ " : "" }}{{ t("out.black") }}
        </button>
        <div class="msep" />
        <button role="menuitem" class="danger" @click="remove">{{ t("row.delete") }}</button>
      </div>
    </div>

    <div v-if="over && canDrop" class="drophint">{{ dropHint }}</div>

    <div v-if="showStats" class="stats">
      <p v-if="!viewers" class="hint">{{ t("st.none") }}</p>
      <p v-else-if="!stats.length" class="hint">{{ t("st.reading") }}</p>
      <dl v-for="(s, i) in stats" :key="i">
        <div><dt>{{ t("st.source") }} {{ stats.length > 1 ? i + 1 : "" }}</dt><dd :class="s.state === 'connected' ? 'good' : 'badv'">{{ stateLabel(s.state) }}</dd></div>
        <div><dt>{{ t("st.inObs") }}</dt><dd>{{ s.program ? t("st.program") : s.visible ? t("st.visible") : t("st.offAir") }}</dd></div>
        <template v-if="s.w">
          <div><dt>{{ t("st.video") }}</dt><dd>{{ s.w }}×{{ s.h }} · {{ Math.round(s.fps ?? 0) }} fps</dd></div>
          <div><dt>{{ t("st.bitrate") }}</dt><dd>{{ mbps(s.kbps) }} Mbps</dd></div>
          <div><dt>{{ t("st.codec") }}</dt><dd :title="s.decoder">{{ s.codec || "?" }}</dd></div>
          <div><dt>{{ t("st.delay") }}</dt><dd>{{ t("st.delayValue", { j: s.jitterMs ?? "?", r: s.rttMs ?? "?" }) }}</dd></div>
          <div class="wide"><dt>{{ t("st.lost") }}</dt><dd :class="{ badv: (s.lost ?? 0) > 0 }">{{ t("st.lostValue", { p: s.lost ?? 0, f: s.dropped ?? 0, z: s.freezes ?? 0 }) }}</dd></div>
        </template>
        <div v-else><dt>{{ t("st.video") }}</dt><dd>{{ t("st.noFrames") }}</dd></div>
        <div><dt>{{ t("st.audio") }}</dt><dd>{{ s.audio ? t("st.yes") : t("st.no") }}</dd></div>
      </dl>
    </div>
  </div>
</template>
