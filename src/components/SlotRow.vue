<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref } from "vue";
import {
  state, thumbs, assign, videoTracks, audioTracks, trackById, slotUrl, outputById,
  updateOutput, moveOutput, deleteOutput, groupOfSlot, outputBg,
} from "../store";

const props = defineProps<{ slot: string | number; fillIndex?: number }>();

const out = computed(() => outputById(props.slot));
const a = computed(() => state.slots[props.slot] ?? { video: null, audio: "auto" });
const video = computed(() => trackById(a.value.video));
const viewers = computed(() => state.viewers[props.slot] ?? 0);
const inGroup = computed(() => props.fillIndex != null);
const copied = ref(false);

const setVideo = (e: Event) => {
  const v = (e.target as HTMLSelectElement).value || null;
  assign(props.slot, { ...a.value, video: v });
};
const setAudio = (e: Event) => {
  const v = (e.target as HTMLSelectElement).value || null;
  assign(props.slot, { ...a.value, audio: v });
};
const copy = async () => {
  await navigator.clipboard.writeText(slotUrl(props.slot));
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
  if (n && n !== out.value?.name) await updateOutput(Number(props.slot), { name: n });
};
const setBg = (bg: "default" | "transparent" | "black") => {
  menu.value = false;
  updateOutput(Number(props.slot), { bg });
};
const bgChoice = computed(() => out.value?.bg ?? "default");
const move = (d: -1 | 1) => moveOutput(Number(props.slot), d);
const remove = () => {
  menu.value = false;
  const g = groupOfSlot(props.slot);
  const extra = g ? `\nEsce anche dal gruppo "${g.name}".` : "";
  if (confirm(`Eliminare l'uscita "${out.value?.name}"? La fonte OBS con questo URL resterà vuota.${extra}`))
    deleteOutput(Number(props.slot));
};
</script>

<template>
  <div class="slot" :class="{ live: video, grouped: inGroup }">
    <div class="num" :title="inGroup ? `Posto ${fillIndex} del gruppo` : `Uscita ${slot}`">
      <small v-if="inGroup">#{{ fillIndex }}</small>
    </div>
    <div class="mini" :class="{ black: outputBg(slot) === 'black' }">
      <img v-if="video && thumbs[video.id]" :src="thumbs[video.id]" alt="" />
    </div>
    <div class="selects">
      <div class="oname">
        <input
          v-if="renaming" ref="nameInput" v-model="draftName" class="rename" maxlength="40"
          @keydown.enter.prevent="commitRename" @keydown.esc.prevent="renaming = false" @blur="commitRename"
        />
        <strong v-else :title="`/slot/${slot}`" @dblclick="startRename">{{ out?.name ?? `Slot ${slot}` }}</strong>
        <span v-if="out?.bg" class="tag" title="Eccezione allo sfondo globale senza video">{{ out.bg === "black" ? "nero" : "trasparente" }}</span>
      </div>
      <select :value="a.video ?? ''" @change="setVideo">
        <option value="">{{ inGroup ? "— vuoto (togli dal gruppo) —" : "— vuoto —" }}</option>
        <option v-if="a.video && !video" :value="a.video">(fuori onda o disconnesso)</option>
        <option v-for="t in videoTracks" :key="t.id" :value="t.id">{{ t.label }}</option>
      </select>
      <select v-if="!inGroup" :value="a.audio ?? ''" @change="setAudio">
        <option value="auto">Audio: automatico</option>
        <option value="">Audio: nessuno</option>
        <option v-for="t in audioTracks" :key="t.id" :value="t.id">Audio: {{ t.label }}</option>
      </select>
    </div>
    <button class="url" :title="slotUrl(slot)" @click="copy">{{ copied ? "Copiato!" : "Copia URL" }}</button>
    <span class="obs" :class="{ on: viewers }" :title="`${viewers} fonti OBS connesse`">OBS {{ viewers }}</span>
    <div class="omenu-wrap">
      <button class="icon" title="Altre azioni" aria-haspopup="menu" :aria-expanded="menu" @click="menu = !menu">⋮</button>
      <div v-if="menu" class="omenu" role="menu">
        <button role="menuitem" @click="startRename">Rinomina</button>
        <button role="menuitem" @click="move(-1)">Sposta su</button>
        <button role="menuitem" @click="move(1)">Sposta giù</button>
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
  </div>
</template>
