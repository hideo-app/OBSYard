<script setup lang="ts">
import { reactive } from "vue";
import { setLowerThird, titleOf, type TrackInfo } from "../store";
import { t } from "../i18n";

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
      <h2>{{ t("ltd.title", { n: track.label }) }}</h2>
      <div class="ltpreview" aria-hidden="true">
        <div class="ltp-name">{{ form.name.trim() || track.label }}</div>
        <div v-if="form.title.trim()" class="ltp-title">{{ form.title }}</div>
      </div>
      <label class="field">{{ t("ltd.name") }}
        <input v-model="form.name" autofocus maxlength="80" :placeholder="track.label" />
      </label>
      <label class="field">{{ t("ltd.second") }} <small>{{ t("ltd.secondNote") }}</small>
        <input v-model="form.title" maxlength="80" :placeholder="t('ltd.secondPh')" />
      </label>
      <p class="hint">{{ t("ltd.saved", { n: track.label }) }}</p>
      <footer class="dfoot">
        <button v-if="saved" type="button" class="danger" @click="reset">{{ t("ltd.reset") }}</button>
        <span class="spacer" />
        <button type="button" class="ghost" @click="emit('close')">{{ t("ltd.cancel") }}</button>
        <button type="submit" class="primary">{{ t("ltd.save") }}</button>
      </footer>
    </form>
  </div>
</template>
