<script setup lang="ts">
import { reactive } from "vue";
import { setLowerThird, titleOf, type TrackInfo } from "../store";

const props = defineProps<{ track: TrackInfo }>();
const emit = defineEmits<{ close: [] }>();

const saved = titleOf(props.track.label);
const form = reactive({ name: saved?.name ?? props.track.label, title: saved?.title ?? "" });

const save = async () => {
  await setLowerThird(props.track.label, form.name.trim() || null, form.title);
  emit("close");
};
const reset = async () => {
  await setLowerThird(props.track.label, null, "");
  emit("close");
};
</script>

<template>
  <div class="backdrop" @click.self="emit('close')" @keydown.esc="emit('close')">
    <form class="dialog small" @submit.prevent="save">
      <h2>Sottopancia di {{ track.label }}</h2>
      <div class="ltpreview" aria-hidden="true">
        <div class="ltp-name">{{ form.name.trim() || track.label }}</div>
        <div v-if="form.title.trim()" class="ltp-title">{{ form.title }}</div>
      </div>
      <label class="field">Nome
        <input v-model="form.name" autofocus maxlength="80" :placeholder="track.label" />
      </label>
      <label class="field">Seconda riga <small>(ruolo, account… facoltativa)</small>
        <input v-model="form.title" maxlength="80" placeholder="es. Giornalista · @mario" />
      </label>
      <p class="hint">Resta salvato per questo nome: vale ogni volta che {{ track.label }} torna in onda.</p>
      <footer class="dfoot">
        <button v-if="saved" type="button" class="danger" @click="reset">Ripristina</button>
        <span class="spacer" />
        <button type="button" class="ghost" @click="emit('close')">Annulla</button>
        <button type="submit" class="primary">Salva</button>
      </footer>
    </form>
  </div>
</template>
