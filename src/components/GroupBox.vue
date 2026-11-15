<script setup lang="ts">
import { computed } from "vue";
import { removeFromGroup, state, firstGroupId, endFill, outputName, groupMove, drag, type GroupView } from "../store";
import { obs } from "../obs";
import { t } from "../i18n";
import SlotRow from "./SlotRow.vue";

const props = defineProps<{ group: GroupView }>();
defineEmits<{ edit: [GroupView] }>();

const standby = computed(() => props.group.members.filter((m) => m.place === "standby"));
const away = computed(() => props.group.members.filter((m) => m.place === "away"));

const status = computed(() => {
  const s = obs.groupStatus[props.group.id];
  switch (s?.kind) {
    case "synced": return { cls: "ok", text: s.set ? `${t("grp.synced")} · ${s.set}` : t("grp.synced"), title: s.set ? t("grp.setInUse", { s: s.set }) : undefined };
    case "away": return { cls: "wait", text: t("grp.away", { s: s.scene }), title: t("grp.awayTitle") };
    case "missing": return { cls: "bad", text: s.set ? `${t("grp.missing")} · ${s.set}` : t("grp.missing"), title: s.items.join("\n") };
    case "offline": return { cls: "bad", text: t("grp.offline") };
    case "off": return { cls: "dim", text: t("grp.off") };
    default: return null;
  }
});

/** un membro in attesa si trascina su un'uscita come una sorgente */
const dragMember = (e: DragEvent, track: string, label: string) => {
  e.dataTransfer!.setData("text/plain", label);
  e.dataTransfer!.effectAllowed = "move";
  drag.kind = "track";
  drag.id = track;
};
</script>

<template>
  <section class="group">
    <header class="ghead">
      <strong>{{ group.name }}</strong>
      <span class="gcount" :title="t('grp.countTitle')">{{ group.count }}/{{ group.slots.length }}</span>
      <span v-if="state.settings.autoAdd && firstGroupId === group.id" class="gpill auto" :title="t('grp.autoTitle')">auto</span>
      <span v-if="status" class="gpill" :class="status.cls" :title="status.title">{{ status.text }}</span>
      <span class="spacer" />
      <button class="icon" :title="t('grp.edit')" @click="$emit('edit', group)">⚙</button>
    </header>

    <SlotRow v-for="(n, i) in group.slots" :key="n" :slot="n" :fill-index="i + 1" />

    <p v-if="group.testFill && group.testFill.slots.length" class="gfill" role="status">
      {{ t("grp.test", { n: group.testFill.names.join(", "), o: outputName(group.testFill.slots[group.testFill.slots.length - 1]) }) }}
      <button class="x" :title="t('grp.endTest', { g: group.name })" @click="endFill(group.id)">×</button>
    </p>
    <p v-if="standby.length || away.length" class="goff" :aria-label="t('grp.notOnSlots')">
      <template v-if="standby.length">
        {{ t("grp.standby") }}
        <span
          v-for="m in standby" :key="m.track" class="gm chipm" draggable="true" :title="t('grp.dragMember')"
          @dragstart="dragMember($event, m.track, m.label)" @dragend="drag.kind = null"
        >
          {{ m.label }}
          <button class="x" :title="t('grp.up', { n: m.label })" @click="groupMove(group.id, m.track, -1)">↑</button>
          <button class="x" :title="t('grp.remove', { n: m.label, g: group.name })" @click="removeFromGroup(group.id, m.track)">×</button>
        </span>
      </template>
      <template v-if="away.length">
        {{ t("grp.away2") }}
        <span v-for="m in away" :key="m.track" class="gm chipm away">
          {{ m.label }}
          <button class="x" :title="t('grp.remove', { n: m.label, g: group.name })" @click="removeFromGroup(group.id, m.track)">×</button>
        </span>
      </template>
    </p>
  </section>
</template>
