<script setup lang="ts">
import { computed, onMounted, reactive, ref, watch } from "vue";
import { state, saveGroup, deleteGroup, outputName, type GroupView, type GroupConfig, type GroupAction } from "../store";
import { obs, refreshCatalog, forceApply, sharedScenes, groupSets, MAIN_SET, SEP } from "../obs";

const props = defineProps<{ group: GroupView | null }>();
const emit = defineEmits<{ close: [] }>();

const allSlots = state.outputs.map((o) => o.id);
const takenBy = (n: number) => state.groups.find((g) => g.id !== props.group?.id && g.slots.includes(n));

/** al massimo 4 set di scene oltre al principale (come guestroom) */
const MAX_SETS = 4;

type FormSet = { name: string; actions: Record<string, { scene: string; items: string[] }>; extra: string };
const toForm = (set: { name: string; actions: Record<string, GroupAction>; extraScenes: string[] }): FormSet => ({
  name: set.name,
  actions: Object.fromEntries(Object.entries(set.actions).map(([k, a]) => [k, { scene: a.scene ?? "", items: [...a.items] }])),
  extra: set.extraScenes.join(", "),
});

const form = reactive({
  name: props.group?.name ?? `Gruppo ${state.groups.length + 1}`,
  slots: [...(props.group?.slots ?? [])] as number[],
  /** set di scene: il principale (0) e gli altri */
  sets: props.group ? groupSets(props.group).map(toForm) : [toForm({ name: MAIN_SET, actions: {}, extraScenes: [] })],
});
/** "set:numero" -> elemento in scrittura */
const draftItem = reactive<Record<string, string>>({});
const error = ref("");
const busy = ref(false);

const counts = computed(() => Array.from({ length: form.slots.length + 1 }, (_, i) => String(i)));
watch(
  [counts, () => form.sets.length],
  ([ks]) => form.sets.forEach((set) => ks.forEach((k) => (set.actions[k] ??= { scene: "", items: [] }))),
  { immediate: true },
);

const addSet = () => {
  const used = new Set(form.sets.map((x) => x.name.toLowerCase()));
  let n = form.sets.length;
  while (used.has(`set ${n}`)) n++;
  form.sets.push({ name: `Set ${n}`, actions: {}, extra: "" });
};
const removeSet = (si: number) => {
  const name = form.sets[si].name;
  if (confirm(`Togliere il set di scene "${name}" e le sue azioni?`)) form.sets.splice(si, 1);
};

const cleanActions = (set: FormSet) =>
  Object.fromEntries(
    counts.value
      .map((k) => [k, { scene: set.actions[k]?.scene.trim() || null, items: set.actions[k]?.items ?? [] }] as const)
      .filter(([, a]) => a.scene || a.items.length),
  );
const cleanExtra = (set: FormSet) => set.extra.split(",").map((x) => x.trim()).filter(Boolean);

/** scene usate da più set: il salvataggio le rifiuta, qui si vedono subito */
const shared = computed(() => sharedScenes(form.sets.map((x) => ({ actions: cleanActions(x), extraScenes: cleanExtra(x) }))));

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
const addItem = (si: number, k: string) => {
  const key = `${si}:${k}`;
  const v = (draftItem[key] ?? "").trim().replace(/\s*>\s*/, SEP);
  if (!v) return;
  const items = form.sets[si].actions[k].items;
  if (!items.includes(v)) items.push(v);
  draftItem[key] = "";
};

const save = async () => {
  error.value = "";
  if (shared.value.length) return void (error.value = `Una scena appartiene a un solo set: ${shared.value.join(", ")}`);
  busy.value = true;
  const [main, ...more] = form.sets;
  const g: GroupConfig = {
    id: props.group?.id ?? 0,
    name: form.name,
    slots: form.slots,
    actions: cleanActions(main),
    extraScenes: cleanExtra(main),
    sets: more.map((x) => ({ name: x.name.trim(), actions: cleanActions(x), extraScenes: cleanExtra(x) })),
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
        <span>Uscite (ordine di riempimento: il partecipante 1 va sulla prima)</span>
        <div class="slotpick">
          <button
            v-for="n in allSlots" :key="n" type="button" class="outbtn"
            :class="{ on: form.slots.includes(n) }" :disabled="!!takenBy(n)"
            :title="takenBy(n) ? `Già nel gruppo ${takenBy(n)!.name}` : outputName(n)"
            @click="toggleSlot(n)"
          >{{ outputName(n) }}</button>
        </div>
        <ol v-if="form.slots.length" class="order">
          <li v-for="(n, i) in form.slots" :key="n">
            <span>#{{ i + 1 }} → {{ outputName(n) }}</span>
            <button type="button" class="icon" :disabled="i === 0" title="Su" @click="move(i, -1)">↑</button>
            <button type="button" class="icon" :disabled="i === form.slots.length - 1" title="Giù" @click="move(i, 1)">↓</button>
          </li>
        </ol>
      </div>

      <div class="field">
        <span>Azioni OBS per numero di partecipanti <small>(facoltative, con "Pilota OBS" attivo)</small></span>
        <p v-if="obs.status !== 'connected'" class="hint">OBS non connesso (si collega solo con "Pilota OBS" attivo): scrivi scene ed elementi come appaiono in OBS.</p>
        <datalist id="obs-scenes"><option v-for="x in obs.scenes" :key="x" :value="x" /></datalist>
        <datalist id="obs-items"><option v-for="x in obs.items" :key="x" :value="x" /></datalist>

        <fieldset v-for="(set, si) in form.sets" :key="si" class="sceneset">
          <legend v-if="form.sets.length > 1">
            <template v-if="si === 0">{{ MAIN_SET }}</template>
            <template v-else>
              <input v-model="set.name" required maxlength="40" aria-label="Nome del set di scene" />
              <button type="button" class="x" :title="`Togli il set ${set.name}`" @click="removeSet(si)">×</button>
            </template>
          </legend>
          <div v-for="k in counts" :key="k" class="action">
            <div class="acount">{{ k }}</div>
            <div v-if="set.actions[k]" class="abody">
              <input v-model="set.actions[k].scene" list="obs-scenes" placeholder="Scena (nessun cambio)" />
              <div class="chips">
                <span v-for="(it, i) in set.actions[k].items" :key="it" class="chip">
                  {{ it }} <button type="button" class="x" @click="set.actions[k].items.splice(i, 1)">×</button>
                </span>
                <input
                  v-model="draftItem[`${si}:${k}`]" list="obs-items" placeholder="Accendi: Scena › Fonte (Invio)"
                  @keydown.enter.prevent="addItem(si, k)" @change="addItem(si, k)"
                />
              </div>
            </div>
          </div>
          <label class="extra">Altre scene {{ si === 0 ? "del gruppo" : "del set" }} <small>(separate da virgola)</small>
            <input v-model="set.extra" placeholder="es. Intervista ravvicinata" />
          </label>
        </fieldset>

        <p class="hint">Gli elementi elencati per qualsiasi numero vengono spenti quando non servono (ogni set spegne i suoi).</p>
        <p v-if="shared.length" class="err">Una scena appartiene a un solo set: {{ shared.join(", ") }}</p>
        <div>
          <button type="button" class="ghost" :disabled="form.sets.length > MAX_SETS" @click="addSet">+ Aggiungi set di scene</button>
        </div>
        <p class="hint">
          Set di scene: per esempio le scene camera "Cam 1/2/3" nel principale e un set "Reaction" con "Reaction 1/2/3".
          Agisce il set che contiene la scena di programma di OBS: su "Reaction 2" un terzo partecipante porta a "Reaction 3".
          Passando a una scena di un altro set, quel set si applica una volta.
        </p>
      </div>

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
