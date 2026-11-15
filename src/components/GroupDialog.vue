<script setup lang="ts">
import { computed, onMounted, reactive, ref, watch } from "vue";
import { state, saveGroup, deleteGroup, type GroupView, type GroupConfig } from "../store";
import { obs, refreshCatalog, forceApply, SEP } from "../obs";

const props = defineProps<{ group: GroupView | null }>();
const emit = defineEmits<{ close: [] }>();

const allSlots = Object.keys(state.slots).map(Number);
const takenBy = (n: number) => state.groups.find((g) => g.id !== props.group?.id && g.slots.includes(n));

const form = reactive({
  name: props.group?.name ?? `Gruppo ${state.groups.length + 1}`,
  slots: [...(props.group?.slots ?? [])] as number[],
  actions: Object.fromEntries(
    Object.entries(props.group?.actions ?? {}).map(([k, a]) => [k, { scene: a.scene ?? "", items: [...a.items] }]),
  ) as Record<string, { scene: string; items: string[] }>,
  extra: (props.group?.extraScenes ?? []).join(", "),
});
const draftItem = reactive<Record<string, string>>({});
const error = ref("");
const busy = ref(false);

const counts = computed(() => Array.from({ length: form.slots.length + 1 }, (_, i) => String(i)));
watch(counts, (ks) => ks.forEach((k) => (form.actions[k] ??= { scene: "", items: [] })), { immediate: true });
const act = (k: string) => form.actions[k];

const toggleSlot = (n: number) => {
  const i = form.slots.indexOf(n);
  if (i >= 0) form.slots.splice(i, 1);
  else form.slots.push(n);
};
const move = (i: number, d: -1 | 1) => {
  const j = i + d;
  if (j < 0 || j >= form.slots.length) return;
  [form.slots[i], form.slots[j]] = [form.slots[j], form.slots[i]];
};
const addItem = (k: string) => {
  const v = (draftItem[k] ?? "").trim().replace(/\s*>\s*/, SEP);
  if (!v) return;
  if (!act(k).items.includes(v)) act(k).items.push(v);
  draftItem[k] = "";
};

const save = async () => {
  error.value = "";
  busy.value = true;
  const g: GroupConfig = {
    id: props.group?.id ?? 0,
    name: form.name,
    slots: form.slots,
    actions: Object.fromEntries(
      counts.value
        .map((k) => [k, { scene: form.actions[k]?.scene.trim() || null, items: form.actions[k]?.items ?? [] }] as const)
        .filter(([, a]) => a.scene || a.items.length),
    ),
    extraScenes: form.extra.split(",").map((s) => s.trim()).filter(Boolean),
  };
  try {
    const id = await saveGroup(g);
    forceApply(id);
    emit("close");
  } catch (e) {
    error.value = String(e);
  } finally {
    busy.value = false;
  }
};

const remove = async () => {
  if (!props.group || !confirm(`Eliminare il gruppo "${props.group.name}"? Gli slot restano con quello che mostrano.`)) return;
  await deleteGroup(props.group.id);
  emit("close");
};

onMounted(() => {
  if (obs.status === "connected") void refreshCatalog();
});
</script>

<template>
  <div class="backdrop" @click.self="emit('close')" @keydown.esc="emit('close')">
    <form class="dialog" @submit.prevent="save">
      <h2>{{ group ? "Modifica gruppo" : "Nuovo gruppo" }}</h2>

      <label class="field">Nome
        <input v-model="form.name" autofocus required maxlength="40" />
      </label>

      <div class="field">
        <span>Slot (ordine di riempimento: il partecipante 1 va sul primo)</span>
        <div class="slotpick">
          <button
            v-for="n in allSlots" :key="n" type="button" class="slotbtn"
            :class="{ on: form.slots.includes(n) }" :disabled="!!takenBy(n)"
            :title="takenBy(n) ? `Già nel gruppo ${takenBy(n)!.name}` : `Slot ${n}`"
            @click="toggleSlot(n)"
          >{{ n }}</button>
        </div>
        <ol v-if="form.slots.length" class="order">
          <li v-for="(n, i) in form.slots" :key="n">
            <span>#{{ i + 1 }} → slot {{ n }}</span>
            <button type="button" class="icon" :disabled="i === 0" title="Su" @click="move(i, -1)">↑</button>
            <button type="button" class="icon" :disabled="i === form.slots.length - 1" title="Giù" @click="move(i, 1)">↓</button>
          </li>
        </ol>
      </div>

      <div class="field">
        <span>Azioni OBS per numero di partecipanti <small>(facoltative, con "Pilota OBS" attivo)</small></span>
        <p v-if="obs.status !== 'connected'" class="hint">OBS non connesso: scrivi scene ed elementi come appaiono in OBS.</p>
        <datalist id="obs-scenes"><option v-for="s in obs.scenes" :key="s" :value="s" /></datalist>
        <datalist id="obs-items"><option v-for="s in obs.items" :key="s" :value="s" /></datalist>
        <div v-for="k in counts" :key="k" class="action">
          <div class="acount">{{ k }}</div>
          <div class="abody">
            <input v-model="act(k).scene" list="obs-scenes" placeholder="Scena (nessun cambio)" />
            <div class="chips">
              <span v-for="(it, i) in act(k).items" :key="it" class="chip">
                {{ it }} <button type="button" class="x" @click="act(k).items.splice(i, 1)">×</button>
              </span>
              <input
                v-model="draftItem[k]" list="obs-items" placeholder="Accendi: Scena › Fonte (Invio)"
                @keydown.enter.prevent="addItem(k)" @change="addItem(k)"
              />
            </div>
          </div>
        </div>
        <p class="hint">Gli elementi elencati per qualsiasi numero vengono spenti quando non servono.</p>
      </div>

      <label class="field">Altre scene del gruppo <small>(separate da virgola)</small>
        <input v-model="form.extra" placeholder="es. Intervista ravvicinata" />
      </label>

      <p v-if="error" class="err">{{ error }}</p>
      <footer class="dfoot">
        <button v-if="group" type="button" class="danger" @click="remove">Elimina</button>
        <span class="spacer" />
        <button type="button" class="ghost" @click="emit('close')">Annulla</button>
        <button type="submit" class="primary" :disabled="busy || !form.slots.length">Salva</button>
      </footer>
    </form>
  </div>
</template>
