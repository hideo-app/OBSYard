// Tipi condivisi tra script iniettato, pagina OBS e UI di controllo.

export type TrackOrigin = "remote" | "camera" | "screen";

export interface TrackInfo {
  id: string;
  kind: "video" | "audio";
  origin: TrackOrigin;
  label: string;
  streamIds: string[];
  muted: boolean;
  w?: number;
  h?: number;
}

export interface SlotAssign {
  /** id traccia video, oppure null */
  video: string | null;
  /** id traccia audio, "auto" (stesso ospite del video) oppure null */
  audio: string | null;
}

export interface Settings {
  codec: "H264" | "VP8" | "VP9" | "AV1";
  maxBitrateKbps: number;
  maxFps: number;
}

export interface GroupAction {
  /** scena di programma da attivare */
  scene?: string | null;
  /** elementi da accendere: "Scena › Fonte" o "Gruppo OBS › Fonte" */
  items: string[];
}

export interface GroupMember {
  track: string;
  label: string;
  /** on = su uno slot, standby = oltre l'ultimo slot, away = non connesso */
  place: "on" | "standby" | "away";
  slot: number | null;
}

export interface GroupConfig {
  id: number;
  name: string;
  /** slot in ordine di riempimento */
  slots: number[];
  /** azioni OBS per numero di partecipanti ("0".."n") */
  actions: Record<string, GroupAction>;
  extraScenes: string[];
}

export interface GroupView extends GroupConfig {
  members: GroupMember[];
  /** partecipanti che contano per le azioni OBS (0..slots.length) */
  count: number;
}

export interface ObsConfig {
  url: string;
  password: string;
  /** Pilota OBS: esegui le azioni dei gruppi */
  drive: boolean;
}

export interface AppState {
  port: number;
  publisherReady: boolean;
  tracks: TrackInfo[];
  slots: Record<string, SlotAssign>;
  viewers: Record<string, number>;
  settings: Settings;
  groups: GroupView[];
  obs: ObsConfig;
}
