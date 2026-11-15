<script setup lang="ts">
import { choice, answerChoice } from "../store";
</script>

<template>
  <div v-if="choice.open" class="backdrop" @click.self="answerChoice(null)" @keydown.esc="answerChoice(null)">
    <div class="dialog small choice" role="alertdialog" aria-modal="true" :aria-label="choice.title">
      <h2>{{ choice.title }}</h2>
      <p v-if="choice.text" class="ctext">{{ choice.text }}</p>
      <p v-if="choice.warn" class="hint warnline">{{ choice.warn }}</p>
      <div class="copts">
        <button
          v-for="(o, i) in choice.options" :key="o.value" type="button" class="copt"
          :class="{ primary: o.primary, danger: o.danger, plain: !o.hint }" :autofocus="i === 0"
          @click="answerChoice(o.value)"
        >
          <strong>{{ o.label }}</strong>
          <small v-if="o.hint">{{ o.hint }}</small>
        </button>
      </div>
    </div>
  </div>
</template>
