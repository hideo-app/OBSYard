<script setup lang="ts">
import { computed } from "vue";
import {
  state, thumbs, assign, groupOfSlot, membershipOf, outputsOnScreen, outputName, selected, drag, trackTally, titleOf,
  type TrackInfo,
} from "../store";
import { t } from "../i18n";

const props = defineProps<{ track: TrackInfo }>();
defineEmits<{ lowerThird: [TrackInfo] }>();

const originLabel = computed(() => ({ remote: t("card.guest"), camera: t("card.local"), screen: t("card.screen") })[props.track.origin]);

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
const slotTitle = (n: string, i: number) => {
  const g = groupOfSlot(n);
  const key = i < 9 ? t("card.shift", { n: i + 1 }) : "";
  return (g ? t("card.sendToGroup", { o: outputName(n), g: g.name }) : t("card.sendTo", { o: outputName(n) })) + key;
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
    class="card" :class="{ assigned: onSlots.length, selected: isSelected, program: tally === 'program', preview: tally === 'preview', audioonly: track.audioOnly }"
    draggable="true" tabindex="0" :aria-pressed="isSelected"
    :title="isSelected ? t('card.selectedTitle') : t('card.title')"
    @click="select" @keydown.enter.self.prevent="select" @keydown.space.self.prevent="select"
    @dragstart="onDragStart" @dragend="onDragEnd"
  >
    <div class="thumb">
      <div v-if="track.audioOnly" class="aonly" :title="t('card.audioOnlyHint')">
        <svg viewBox="0 0 24 24" width="34" height="34" aria-hidden="true"><path fill="currentColor" d="M12 15a3 3 0 0 0 3-3V6a3 3 0 1 0-6 0v6a3 3 0 0 0 3 3Zm5-3a5 5 0 0 1-10 0H5a7 7 0 0 0 6 6.92V21h2v-2.08A7 7 0 0 0 19 12h-2Z" /></svg>
        <span>{{ t("card.audioOnly") }}</span>
      </div>
      <img v-else-if="thumbs[track.id]" :src="thumbs[track.id]" alt="" draggable="false" />
      <span v-else>{{ track.muted ? t("card.noSignal") : "…" }}</span>
      <span class="badge" :class="track.origin">{{ originLabel }}</span>
      <span v-if="tally === 'program'" class="onair">{{ t("card.onAir") }}</span>
      <span v-else-if="tally === 'preview'" class="onair pre">{{ t("card.preview") }}</span>
      <span v-if="track.w" class="res">{{ track.w }}×{{ track.h }}</span>
    </div>
    <div class="meta">
      <div class="who">
        <strong :title="track.id">{{ shownName }}</strong>
        <small v-if="lt?.title">{{ lt.title }}</small>
        <small v-else-if="membership" class="memb">{{ membership.group.name }} · {{ t(`place.${membership.member.place}`) }}</small>
      </div>
      <span v-if="track.muted && !track.audioOnly" class="warn">{{ t("card.paused") }}</span>
      <button
        v-if="state.settings.lowerThirds !== 'off'" class="icon ltbtn" :class="{ set: lt }"
        :title="t('card.lt')" @click.stop="$emit('lowerThird', track)"
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
