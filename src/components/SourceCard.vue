<script setup lang="ts">
import { computed } from "vue";
import {
  state, thumbs, assign, groupOfSlot, membershipOf, outputsOnScreen, outputName, selected, drag, trackTally, titleOf,
  type TrackInfo,
} from "../store";

const props = defineProps<{ track: TrackInfo }>();
defineEmits<{ lowerThird: [TrackInfo] }>();

const originLabel = { remote: "Ospite", camera: "Locale", screen: "Schermo" } as const;

/** slot in cui questa traccia è attualmente in onda */
const onSlots = computed(() =>
  Object.entries(state.slots).filter(([, a]) => a.video === props.track.id).map(([n]) => n),
);
const tally = computed(() => trackTally(props.track.id));
const isSelected = computed(() => selected.value === props.track.id);

const toggle = (n: string) => {
  const cur = state.slots[n] ?? { video: null, audio: "auto" };
  const on = cur.video === props.track.id;
  assign(n, { video: on ? null : props.track.id, audio: cur.audio ?? "auto" });
};
const select = () => (selected.value = isSelected.value ? null : props.track.id);

const membership = computed(() => membershipOf(props.track.id));
const placeLabel = { on: "in onda", standby: "in attesa", away: "assente" } as const;
const slotTitle = (n: string, i: number) => {
  const g = groupOfSlot(n);
  const key = i < 9 ? ` · Shift+${i + 1} sulla sorgente selezionata` : "";
  return (g ? `Manda a ${outputName(n)} (gruppo ${g.name})` : `Manda a ${outputName(n)}`) + key;
};

/** sottopancia salvato per questa persona (nome diverso o seconda riga) */
const lt = computed(() => titleOf(props.track.label));
const shownName = computed(() => lt.value?.name || props.track.label);

const onDragStart = (e: DragEvent) => {
  e.dataTransfer!.setData("text/plain", props.track.label);
  e.dataTransfer!.effectAllowed = "copyMove";
  drag.kind = "track";
  drag.id = props.track.id;
};
const onDragEnd = () => (drag.kind = null);
</script>

<template>
  <article
    class="card" :class="{ assigned: onSlots.length, selected: isSelected, program: tally === 'program', preview: tally === 'preview' }"
    draggable="true" tabindex="0" :aria-pressed="isSelected"
    :title="isSelected ? 'Selezionata: Shift+1…9 la manda su un\'uscita, Esc deseleziona' : 'Clic per selezionare, trascina su un\'uscita'"
    @click="select" @keydown.enter.self.prevent="select" @keydown.space.self.prevent="select"
    @dragstart="onDragStart" @dragend="onDragEnd"
  >
    <div class="thumb">
      <img v-if="thumbs[track.id]" :src="thumbs[track.id]" alt="" draggable="false" />
      <span v-else>{{ track.muted ? "nessun segnale" : "…" }}</span>
      <span class="badge" :class="track.origin">{{ originLabel[track.origin] }}</span>
      <span v-if="tally === 'program'" class="onair">ON AIR</span>
      <span v-else-if="tally === 'preview'" class="onair pre">ANTEPRIMA</span>
      <span v-if="track.w" class="res">{{ track.w }}×{{ track.h }}</span>
    </div>
    <div class="meta">
      <div class="who">
        <strong :title="track.id">{{ shownName }}</strong>
        <small v-if="lt?.title">{{ lt.title }}</small>
        <small v-else-if="membership" class="memb">{{ membership.group.name }} · {{ placeLabel[membership.member.place] }}</small>
      </div>
      <span v-if="track.muted" class="warn">in pausa</span>
      <button
        v-if="state.settings.lowerThirds !== 'off'" class="icon ltbtn" :class="{ set: lt }"
        title="Sottopancia: nome e seconda riga" @click.stop="$emit('lowerThird', track)"
      >Aa</button>
    </div>
    <div class="slots" @click.stop>
      <button
        v-for="(n, i) in outputsOnScreen"
        :key="n"
        class="slotbtn"
        :class="{ on: onSlots.includes(String(n)), grp: groupOfSlot(n) }"
        :title="slotTitle(String(n), i)"
        @click="toggle(String(n))"
      >{{ i + 1 }}</button>
    </div>
  </article>
</template>
