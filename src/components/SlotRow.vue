<script setup lang="ts">
import { computed, ref } from "vue";
import { state, thumbs, assign, videoTracks, audioTracks, trackById, slotUrl } from "../store";

const props = defineProps<{ slot: string | number; fillIndex?: number }>();

const a = computed(() => state.slots[props.slot]);
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
</script>

<template>
  <div class="slot" :class="{ live: video, grouped: inGroup }">
    <div class="num" :title="inGroup ? `Slot ${slot}, posto ${fillIndex} del gruppo` : `Slot ${slot}`">
      {{ slot }}<small v-if="inGroup">#{{ fillIndex }}</small>
    </div>
    <div class="mini">
      <img v-if="video && thumbs[video.id]" :src="thumbs[video.id]" alt="" />
    </div>
    <div class="selects">
      <select :value="a.video ?? ''" @change="setVideo">
        <option value="">{{ inGroup ? "— vuoto (togli dal gruppo) —" : "— vuoto —" }}</option>
        <option v-if="a.video && !video" :value="a.video">(disconnesso, in attesa)</option>
        <option v-for="t in videoTracks" :key="t.id" :value="t.id">{{ t.label }}</option>
      </select>
      <select v-if="!inGroup" :value="a.audio ?? ''" @change="setAudio">
        <option value="auto">Audio: automatico</option>
        <option value="">Audio: nessuno</option>
        <option v-for="t in audioTracks" :key="t.id" :value="t.id">Audio: {{ t.label }}</option>
      </select>
      <span v-else class="auto">Audio: automatico</span>
    </div>
    <button class="url" :title="slotUrl(slot)" @click="copy">{{ copied ? "Copiato!" : "Copia URL" }}</button>
    <span class="obs" :class="{ on: viewers }" :title="`${viewers} fonti OBS connesse`">OBS {{ viewers }}</span>
  </div>
</template>
