<script setup lang="ts">
import { computed } from "vue";
import { removeFromGroup, state, firstGroupId, endFill, outputName, type GroupView } from "../store";
import { obs } from "../obs";
import SlotRow from "./SlotRow.vue";

const props = defineProps<{ group: GroupView }>();
defineEmits<{ edit: [GroupView] }>();

const standby = computed(() => props.group.members.filter((m) => m.place === "standby"));
const away = computed(() => props.group.members.filter((m) => m.place === "away"));

const status = computed(() => {
  const s = obs.groupStatus[props.group.id];
  switch (s?.kind) {
    case "synced": return { cls: "ok", text: s.set ? `OBS allineato · ${s.set}` : "OBS allineato", title: s.set ? `Set di scene in uso: ${s.set}` : undefined };
    case "away": return { cls: "wait", text: `OBS su "${s.scene}"`, title: "La scena di programma non è una scena del gruppo: si allinea quando ci torni" };
    case "missing": return { cls: "bad", text: s.set ? `Elementi mancanti · ${s.set}` : "Elementi mancanti", title: s.items.join("\n") };
    case "offline": return { cls: "bad", text: "OBS offline" };
    case "off": return { cls: "dim", text: "Pilota OBS spento" };
    default: return null;
  }
});
</script>

<template>
  <section class="group">
    <header class="ghead">
      <strong>{{ group.name }}</strong>
      <span class="gcount" title="Partecipanti che contano per le azioni OBS">{{ group.count }}/{{ group.slots.length }}</span>
      <span v-if="state.settings.autoAdd && firstGroupId === group.id" class="gpill auto" title="Chi va in onda entra qui in automatico">auto</span>
      <span v-if="status" class="gpill" :class="status.cls" :title="status.title">{{ status.text }}</span>
      <span class="spacer" />
      <button class="icon" title="Modifica gruppo" @click="$emit('edit', group)">⚙</button>
    </header>

    <SlotRow v-for="(n, i) in group.slots" :key="n" :slot="n" :fill-index="i + 1" />

    <p v-if="group.testFill && group.testFill.slots.length" class="gfill" role="status">
      ⚠ Prova: {{ group.testFill.names.join(", ") }} sulle uscite fino a {{ outputName(group.testFill.slots[group.testFill.slots.length - 1]) }}
      <button class="x" :title="`Termina la prova di ${group.name}`" @click="endFill(group.id)">×</button>
    </p>
    <p v-if="standby.length || away.length" class="goff" aria-label="Membri non sugli slot">
      <template v-if="standby.length">
        In attesa:
        <span v-for="m in standby" :key="m.track" class="gm">
          {{ m.label }}
          <button class="x" :title="`Togli ${m.label} da ${group.name}`" @click="removeFromGroup(group.id, m.track)">×</button>
        </span>
      </template>
      <template v-if="standby.length && away.length"> · </template>
      <template v-if="away.length">
        Assenti:
        <span v-for="m in away" :key="m.track" class="gm away">
          {{ m.label }}
          <button class="x" :title="`Togli ${m.label} da ${group.name}`" @click="removeFromGroup(group.id, m.track)">×</button>
        </span>
      </template>
    </p>
  </section>
</template>
