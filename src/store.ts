import { reactive, computed } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  AppState, GroupConfig, GroupView, ObsConfig, Settings, SlotAssign, TrackInfo,
} from "../src-inject/types";

export type { AppState, GroupConfig, GroupView, ObsConfig, Settings, SlotAssign, TrackInfo };

export const state = reactive<AppState>({
  port: 7878,
  publisherReady: false,
  tracks: [],
  slots: {},
  viewers: {},
  settings: { codec: "H264", maxBitrateKbps: 12000, maxFps: 60 },
  groups: [],
  obs: { url: "ws://127.0.0.1:4455", password: "", drive: false },
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

/**
 * Ordine della colonna slot: ogni gruppo è un blocco con i suoi slot in ordine di
 * riempimento, messo dove sta il suo primo slot; gli altri slot restano al loro posto.
 */
export const slotLayout = computed(() => {
  const out: ({ kind: "slot"; n: number } | { kind: "group"; group: GroupView })[] = [];
  const placed = new Set<number>();
  for (const key of Object.keys(state.slots)) {
    const n = Number(key);
    const g = groupOfSlot(n);
    if (!g) out.push({ kind: "slot", n });
    else if (!placed.has(g.id) && g.slots[0] === n) {
      placed.add(g.id);
      out.push({ kind: "group", group: g });
    }
  }
  // gruppi il cui primo slot non è nell'elenco (non dovrebbe succedere)
  for (const g of state.groups) if (!placed.has(g.id)) out.push({ kind: "group", group: g });
  return out;
});

export const slotUrl = (n: number | string) => `http://127.0.0.1:${state.port}/slot/${n}`;

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

export const assign = (slot: number | string, a: SlotAssign) => invoke("assign", { slot: Number(slot), assign: a });

export const setSettings = (s: Settings) => invoke("set_settings", { settings: s });

export const saveGroup = (group: GroupConfig) => invoke<number>("group_save", { group });
export const deleteGroup = (id: number) => invoke("group_delete", { id });
export const removeFromGroup = (id: number, track: string) => invoke("group_remove", { id, track });
export const setObs = (obs: ObsConfig) => invoke("set_obs", { obs });

export const studio = (action: "show" | "reload" | "devtools" | "navigate", url?: string) =>
  invoke("studio_action", { action, url });
