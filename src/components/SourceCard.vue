<script setup lang="ts">
import { computed } from "vue";
import { state, thumbs, assign, groupOfSlot, membershipOf, outputsOnScreen, outputName, type TrackInfo } from "../store";

const props = defineProps<{ track: TrackInfo }>();

const originLabel = { remote: "Ospite", camera: "Locale", screen: "Schermo" } as const;

/** slot in cui questa traccia è attualmente in onda */
const onSlots = computed(() =>
  Object.entries(state.slots).filter(([, a]) => a.video === props.track.id).map(([n]) => n),
);

const toggle = (n: string) => {
  const cur = state.slots[n] ?? { video: null, audio: "auto" };
  const on = cur.video === props.track.id;
  assign(n, { video: on ? null : props.track.id, audio: cur.audio ?? "auto" });
};

const membership = computed(() => membershipOf(props.track.id));
const placeLabel = { on: "in onda", standby: "in attesa", away: "assente" } as const;
const slotTitle = (n: string) => {
  const g = groupOfSlot(n);
  return g ? `Manda a ${outputName(n)} (gruppo ${g.name})` : `Manda a ${outputName(n)}`;
};
</script>

<template>
  <article class="card" :class="{ live: onSlots.length }">
    <div class="thumb">
      <img v-if="thumbs[track.id]" :src="thumbs[track.id]" alt="" />
      <span v-else>{{ track.muted ? "nessun segnale" : "…" }}</span>
      <span class="badge" :class="track.origin">{{ originLabel[track.origin] }}</span>
      <span v-if="track.w" class="res">{{ track.w }}×{{ track.h }}</span>
    </div>
    <div class="meta">
      <strong :title="track.id">{{ track.label }}</strong>
      <span v-if="track.muted" class="warn">in pausa</span>
    </div>
    <div v-if="membership" class="memb">
      {{ membership.group.name }} · {{ placeLabel[membership.member.place] }}
    </div>
    <div class="slots">
      <button
        v-for="(n, i) in outputsOnScreen"
        :key="n"
        class="slotbtn"
        :class="{ on: onSlots.includes(String(n)), grp: groupOfSlot(n) }"
        :title="slotTitle(String(n))"
        @click="toggle(String(n))"
      >{{ i + 1 }}</button>
    </div>
  </article>
</template>
