<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watch } from "vue";
import {
  state, thumbs, assign, videoTracks, audioTracks, trackById, slotUrl, slotNameUrl, outputById,
  updateOutput, moveOutput, deleteOutput, groupOfSlot, outputBg, tallyOf, drag, sendTo, swapOutputs,
  groupMove, outputStats, outputName, outputsOnScreen, alsoOn, type OutputStats,
} from "../store";
import { obs } from "../obs";

const props = defineProps<{ slot: string | number; fillIndex?: number }>();

const id = computed(() => Number(props.slot));
const out = computed(() => outputById(props.slot));
const a = computed(() => state.slots[props.slot] ?? { video: null, audio: "auto" });
const video = computed(() => trackById(a.value.video));
const viewers = computed(() => state.viewers[props.slot] ?? 0);
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
  const grouped = state.dups.includes(id.value)
    ? ". Duplicato nel gruppo: sparisce svuotando l'uscita, mandandoci un'altra sorgente o quando ci arriva un membro"
    : "";
  return `La stessa sorgente è anche su ${where}${grouped}`;
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
  const extra = g ? `\nEsce anche dal gruppo "${g.name}".` : "";
  if (confirm(`Eliminare l'uscita "${out.value?.name}"? La fonte OBS con questo URL resterà vuota.${extra}`))
    deleteOutput(id.value);
};

// ---------- trascinamento: sorgente → uscita, uscita → uscita ----------
const over = ref(false);
const canDrop = computed(() => drag.kind === "track" || (drag.kind === "output" && drag.id !== String(props.slot)));
/** cosa farà il rilascio, mostrato sull'uscita sotto il puntatore */
const dropHint = computed(() => {
  if (drag.kind === "track") {
    if (a.value.video === drag.id) return "È già qui";
    if (group.value?.members.some((m) => m.track === drag.id)) return "Scambia di posto";
    return inGroup.value ? "Inserisci qui (il gruppo fa posto)" : video.value ? "Sostituisci" : "Manda qui";
  }
  if (drag.kind === "output") {
    const from = groupOfSlot(drag.id);
    if (!from && !group.value) return "Scambia le due uscite";
    if (from && from.id === group.value?.id) return "Scambia di posto";
    return `Manda qui la sorgente di ${outputName(drag.id)}`;
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
  ({ connected: "collegata", connecting: "connessione…", new: "connessione…", failed: "fallita", disconnected: "interrotta", closed: "chiusa" })[s] ?? s;
const mbps = (k?: number) => (k == null ? "…" : (k / 1000).toFixed(1));
</script>

<template>
  <div
    class="slot" :class="{ live: video, grouped: inGroup, program: tally === 'program', preview: tally === 'preview', over: over && canDrop, droppable: canDrop }"
    tabindex="0"
    @keydown.alt.up.self.prevent="altArrow(-1)" @keydown.alt.down.self.prevent="altArrow(1)"
    @dragover="onDragOver" @dragleave="onDragLeave" @drop="onDrop"
  >
    <div class="num" :title="(inGroup ? `Posto ${fillIndex} del gruppo · ` : '') + (screenIndex <= 9 ? `Uscita ${screenIndex}: Shift+${screenIndex} manda qui la sorgente selezionata` : `Uscita ${screenIndex}`)">
      {{ screenIndex }}
    </div>
    <div
      class="mini" :class="{ black: outputBg(slot) === 'black', grab: !!a.video }" :draggable="!!a.video"
      :title="a.video ? 'Trascina su un\'altra uscita per scambiare' : undefined"
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
        <strong v-else :title="`/slot/${slot} · doppio clic per rinominare`" @dblclick="startRename">{{ out?.name ?? `Slot ${slot}` }}</strong>
        <span v-if="tally === 'program'" class="tag air" title="Questa uscita è in programma in OBS">ON AIR</span>
        <span v-else-if="tally === 'preview'" class="tag pre" title="Questa uscita è visibile in OBS (anteprima), non in programma">ANTEPRIMA</span>
        <span v-if="dupOf.length" class="tag dup" :title="dupTitle">duplicato</span>
        <span v-if="inTest" class="tag test" title="Prova di riempimento: termina svuotando questa uscita o con la × sotto il gruppo">prova</span>
        <span v-if="out?.bg" class="tag" title="Eccezione allo sfondo globale senza video">{{ out.bg === "black" ? "nero" : "trasparente" }}</span>
        <span v-if="ltMissing.length" class="tag bad" :title="`In OBS mancano le fonti Testo:\n${ltMissing.join('\n')}\nCreale da Sottopancia → Crea in OBS`">testo mancante</span>
      </div>
      <select :value="a.video ?? ''" aria-label="Sorgente video" @change="setVideo">
        <option value="">{{ inGroup ? "— vuoto (togli dal gruppo) —" : "— vuoto —" }}</option>
        <option v-if="a.video && !video" :value="a.video">(fuori onda o disconnesso)</option>
        <option v-for="t in videoTracks" :key="t.id" :value="t.id">{{ t.label }}</option>
      </select>
      <select v-if="!inGroup" :value="a.audio ?? ''" aria-label="Sorgente audio" @change="setAudio">
        <option value="auto">Audio: automatico</option>
        <option value="">Audio: nessuno</option>
        <option v-for="t in audioTracks" :key="t.id" :value="t.id">Audio: {{ t.label }}</option>
      </select>
      <div v-if="lt" class="ltline" :title="state.settings.lowerThirds === 'obs' ? 'Sottopancia nelle fonti Testo di OBS' : 'Sottopancia disegnato sull\'uscita'">
        <span class="ltmark">Aa</span> {{ lt.name }}<template v-if="lt.title"> · {{ lt.title }}</template>
      </div>
    </div>
    <button class="url" :title="`${slotUrl(slot)}\nCon il nome: ${slotNameUrl(slot)} (menu ⋮)`" @click="copy()">{{ copied ? "Copiato!" : "Copia URL" }}</button>
    <button
      class="obs" :class="{ on: viewers, open: showStats }" :aria-expanded="showStats"
      :title="viewers ? `${viewers} fonti OBS collegate: clic per i dettagli della connessione` : 'Nessuna fonte OBS ha aperto questa uscita'"
      @click="openStats"
    ><span class="led" />OBS {{ viewers }}</button>
    <div class="omenu-wrap">
      <button class="icon" title="Altre azioni" aria-haspopup="menu" :aria-expanded="menu" @click="menu = !menu">⋮</button>
      <div v-if="menu" class="omenu" role="menu">
        <button role="menuitem" @click="startRename">Rinomina</button>
        <button role="menuitem" :title="slotNameUrl(slot)" @click="copy(true)">Copia URL con il nome</button>
        <template v-if="inGroup && a.video">
          <button role="menuitem" @click="movePerson(-1)">Persona su <kbd>Alt+↑</kbd></button>
          <button role="menuitem" @click="movePerson(1)">Persona giù <kbd>Alt+↓</kbd></button>
        </template>
        <button role="menuitem" @click="move(-1)">Sposta uscita su <kbd v-if="!inGroup">Alt+↑</kbd></button>
        <button role="menuitem" @click="move(1)">Sposta uscita giù <kbd v-if="!inGroup">Alt+↓</kbd></button>
        <button role="menuitem" @click="openStats">{{ showStats ? "Nascondi" : "Mostra" }} dettagli connessione</button>
        <button v-if="state.settings.lowerThirds !== 'off'" role="menuitemcheckbox" :aria-checked="!!out?.lt" @click="toggleLt">
          {{ out?.lt ? "✓ " : "" }}Sottopancia su questa uscita
        </button>
        <div class="msep">Senza video</div>
        <button role="menuitemradio" :aria-checked="bgChoice === 'default'" @click="setBg('default')">
          {{ bgChoice === "default" ? "✓ " : "" }}Come impostazione ({{ state.settings.emptyBg === "black" ? "nero" : "trasparente" }})
        </button>
        <button role="menuitemradio" :aria-checked="bgChoice === 'transparent'" @click="setBg('transparent')">
          {{ bgChoice === "transparent" ? "✓ " : "" }}Trasparente
        </button>
        <button role="menuitemradio" :aria-checked="bgChoice === 'black'" @click="setBg('black')">
          {{ bgChoice === "black" ? "✓ " : "" }}Nero
        </button>
        <div class="msep" />
        <button role="menuitem" class="danger" @click="remove">Elimina…</button>
      </div>
    </div>

    <div v-if="over && canDrop" class="drophint">{{ dropHint }}</div>

    <div v-if="showStats" class="stats">
      <p v-if="!viewers" class="hint">Nessuna fonte OBS ha aperto questa uscita: aggiungi in OBS una Fonte browser con il suo URL.</p>
      <p v-else-if="!stats.length" class="hint">Leggo la connessione…</p>
      <dl v-for="(s, i) in stats" :key="i">
        <div><dt>Fonte {{ stats.length > 1 ? i + 1 : "" }}</dt><dd :class="s.state === 'connected' ? 'good' : 'badv'">{{ stateLabel(s.state) }}</dd></div>
        <div><dt>In OBS</dt><dd>{{ s.program ? "in programma" : s.visible ? "visibile (anteprima)" : "non in onda" }}</dd></div>
        <template v-if="s.w">
          <div><dt>Video</dt><dd>{{ s.w }}×{{ s.h }} · {{ Math.round(s.fps ?? 0) }} fps</dd></div>
          <div><dt>Bitrate</dt><dd>{{ mbps(s.kbps) }} Mbps</dd></div>
          <div><dt>Codec</dt><dd :title="s.decoder">{{ s.codec || "?" }}</dd></div>
          <div><dt>Ritardo</dt><dd>buffer {{ s.jitterMs ?? "?" }} ms · RTT {{ s.rttMs ?? "?" }} ms</dd></div>
          <div class="wide"><dt>Persi</dt><dd :class="{ badv: (s.lost ?? 0) > 0 }">{{ s.lost ?? 0 }} pacchetti · {{ s.dropped ?? 0 }} fotogrammi · {{ s.freezes ?? 0 }} blocchi</dd></div>
        </template>
        <div v-else><dt>Video</dt><dd>nessun fotogramma</dd></div>
        <div><dt>Audio</dt><dd>{{ s.audio ? "sì" : "no" }}</dd></div>
      </dl>
    </div>
  </div>
</template>
