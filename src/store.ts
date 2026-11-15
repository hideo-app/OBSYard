import { reactive, computed, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  AppState, GroupAction, GroupConfig, GroupSceneSet, GroupView, LowerThird, ObsConfig, Output, Settings, SlotAssign, TrackInfo,
} from "../src-inject/types";

export type { AppState, GroupAction, GroupConfig, GroupSceneSet, GroupView, LowerThird, ObsConfig, Output, Settings, SlotAssign, TrackInfo };

export const state = reactive<AppState>({
  port: 7878,
  publisherReady: false,
  tracks: [],
  outputs: [],
  slots: {},
  viewers: {},
  tally: {},
  lowerThirds: {},
  titles: {},
  settings: { codec: "H264", maxBitrateKbps: 12000, maxFps: 60, fullResLayout: "OBSYard", autoAdd: false, emptyBg: "transparent", priority: [], lowerThirds: "off" },
  groups: [],
  obs: { url: "ws://127.0.0.1:4455", password: "", drive: false },
  fullRes: { on: false, status: "off", layout: "OBSYard" },
});

/** miniature: id traccia -> data URL jpeg (aggiornate ogni ~2s dallo studio) */
export const thumbs = reactive<Record<string, string>>({});

export const videoTracks = computed(() => state.tracks.filter((t) => t.kind === "video"));
export const audioTracks = computed(() => state.tracks.filter((t) => t.kind === "audio"));
export const trackById = (id: string | null | undefined) => state.tracks.find((t) => t.id === id);
export const groupOfSlot = (slot: number | string) => state.groups.find((g) => g.slots.includes(Number(slot)));

/** gruppo di cui la traccia è membro, con il suo posto */
export const membershipOf = (track: string) => {
  for (const g of state.groups) {
    const m = g.members.find((m) => m.track === track);
    if (m) return { group: g, member: m };
  }
  return null;
};

export const outputById = (id: number | string) => state.outputs.find((o) => o.id === Number(id));
export const outputName = (id: number | string) => outputById(id)?.name ?? `Slot ${id}`;

/**
 * Ordine della colonna uscite (come guestroom): l'ordine delle uscite, con ogni gruppo come
 * blocco dei suoi slot in ordine di riempimento, messo dove sta la sua prima uscita.
 */
export const slotLayout = computed(() => {
  const out: ({ kind: "slot"; n: number } | { kind: "group"; group: GroupView })[] = [];
  const placed = new Set<number>();
  for (const o of state.outputs) {
    const g = groupOfSlot(o.id);
    if (!g) out.push({ kind: "slot", n: o.id });
    else if (!placed.has(g.id)) {
      placed.add(g.id);
      out.push({ kind: "group", group: g });
    }
  }
  return out;
});

/** il primo gruppo a schermo: quello in cui entrano i partecipanti con l'aggiunta automatica */
export const firstGroupId = computed(() => {
  for (const it of slotLayout.value) if (it.kind === "group") return it.group.id;
  return null;
});

/** uscite nell'ordine in cui compaiono a schermo (gruppi compresi): serve a numerarle 1…N */
export const outputsOnScreen = computed(() =>
  slotLayout.value.flatMap((it) => (it.kind === "slot" ? [it.n] : it.group.slots)),
);

// ------------------------------------------------------------ tally (ON AIR)
/** tally di un'uscita: in programma in OBS, solo visibile (anteprima) o niente */
export const tallyOf = (slot: number | string) => state.tally[String(slot)] ?? null;
/** tally di una sorgente: il più alto tra le uscite che la mostrano */
export const trackTally = (track: string) => {
  let best: "program" | "preview" | null = null;
  for (const [n, a] of Object.entries(state.slots)) {
    if (a.video !== track) continue;
    const t = tallyOf(n);
    if (t === "program") return "program";
    if (t) best = t;
  }
  return best;
};

// ------------------------------------------------- selezione e trascinamento
/** sorgente selezionata (clic sulla card): Shift+1…9 la manda sull'uscita N */
export const selected = ref<string | null>(null);
/** cosa si sta trascinando: una sorgente (card, membro in attesa) o un'uscita */
export const drag = reactive<{ kind: "track" | "output" | null; id: string }>({ kind: null, id: "" });

/** chiave dei sottopancia salvati: nome minuscolo con gli spazi normalizzati (come in Rust) */
export const titleKey = (label: string) => label.trim().split(/\s+/).join(" ").toLowerCase();
export const titleOf = (label: string): LowerThird | undefined => state.titles[titleKey(label)];

export const slotUrl = (n: number | string) => `http://127.0.0.1:${state.port}/slot/${n}`;
/** alternativa: l'URL con il nome dell'uscita (vale finché non la rinomini) */
export const slotNameUrl = (n: number | string) =>
  `http://127.0.0.1:${state.port}/slot/${encodeURIComponent(outputName(n))}`;

export async function init() {
  await listen<AppState>("state", (e) => Object.assign(state, e.payload));
  await listen<Record<string, string>>("thumbs", (e) => {
    const live = new Set(state.tracks.map((t) => t.id));
    for (const k of Object.keys(thumbs)) if (!live.has(k)) delete thumbs[k];
    Object.assign(thumbs, e.payload);
  });
  Object.assign(state, await invoke<AppState>("get_state"));

  // niente miniature quando la regia non è visibile: zero lavoro inutile nello studio
  const syncThumbs = () => invoke("set_thumbs", { on: document.visibilityState === "visible" });
  document.addEventListener("visibilitychange", syncThumbs);
  syncThumbs();
}

/**
 * Manda una sorgente su un'uscita. Su un'uscita di gruppo oltre la prima libera OBSYard chiede
 * (come guestroom) se avviare la prova di riempimento: la persona va lì e sulle uscite libere prima.
 */
export const assign = async (slot: number | string, a: SlotAssign) => {
  const r = await invoke<{ ask?: { group: string; name: string; chosen: number; landed: number } } | null>("assign", {
    slot: Number(slot),
    assign: a,
  });
  if (!r?.ask) return;
  const { group, name, chosen, landed } = r.ask;
  const who = name || "questa sorgente";
  const ok = confirm(
    `${outputName(chosen)} è oltre le persone in ${group}: il gruppo riempie le uscite in ordine, quindi ${who} andrebbe su ${outputName(landed)}.\n\n` +
      `Riempire comunque tutte le uscite fino a ${outputName(chosen)} con ${who}? Utile per provare da solo le scene con più persone.\n\n` +
      `OK = riempi fino a ${outputName(chosen)} (prova) · Annulla = lascia com'è`,
  );
  if (ok) await invoke("assign", { slot: Number(slot), assign: a, fill: true });
};

/** manda una sorgente su un'uscita tenendo l'audio che l'uscita ha già scelto */
export const sendTo = (slot: number | string, track: string) =>
  assign(slot, { video: track, audio: state.slots[String(slot)]?.audio ?? "auto" });

/** trascinare un'uscita su un'altra: scambio (o invio della sorgente se i gruppi sono diversi) */
export const swapOutputs = (a: number, b: number) => invoke("swap_outputs", { a, b });
export const groupMove = (id: number, track: string, delta: -1 | 1) => invoke("group_move", { id, track, delta });
export const setLowerThird = (label: string, name: string | null, title: string) => invoke("set_lower_third", { label, name, title });

export interface OutputStats {
  state: string; program: boolean; visible: boolean; audio?: boolean;
  w?: number; h?: number; fps?: number; kbps?: number; codec?: string; decoder?: string;
  jitterMs?: number; lost?: number; dropped?: number; freezes?: number; rttMs?: number;
}
/** dettagli della connessione delle fonti OBS di un'uscita (una voce per fonte collegata) */
export const outputStats = (slot: number) => invoke<OutputStats[]>("output_stats", { slot });

export const endFill = (id: number) => invoke("group_end_fill", { id });

export const setSettings = (s: Settings) => invoke("set_settings", { settings: s });

export const saveGroup = (group: GroupConfig) => invoke<number>("group_save", { group });
export const deleteGroup = (id: number) => invoke("group_delete", { id });
export const removeFromGroup = (id: number, track: string) => invoke("group_remove", { id, track });
export const setObs = (obs: ObsConfig) => invoke("set_obs", { obs });
export const setFullRes = (on: boolean) => invoke("set_full_res", { on });

export const addOutput = (name?: string) => invoke<number>("output_add", { name: name ?? null });
export const updateOutput = (id: number, patch: { name?: string; bg?: "default" | "transparent" | "black"; lt?: boolean }) =>
  invoke("output_update", { id, name: patch.name ?? null, bg: patch.bg ?? null, lt: patch.lt ?? null });
/** sfondo effettivo di un'uscita senza video (eccezione o impostazione globale) */
export const outputBg = (id: number | string) => outputById(id)?.bg ?? state.settings.emptyBg;
export const moveOutput = (id: number, delta: -1 | 1) => invoke("output_move", { id, delta });
export const deleteOutput = (id: number) => invoke("output_delete", { id });
/** prossimo nome libero "Cam N" (precompilato nel modulo Aggiungi uscita) */
export const nextOutputName = () =>
  `Cam ${Math.max(0, ...state.outputs.map((o) => Number(/^Cam (\d+)$/.exec(o.name)?.[1] ?? 0))) + 1}`;

export const studio = (action: "show" | "reload" | "devtools" | "navigate", url?: string) =>
  invoke("studio_action", { action, url });
