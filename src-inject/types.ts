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
  /** layout personalizzato di StreamYard con tutti i riquadri a piena altezza */
  fullResLayout: string;
  /** chi va in onda entra da solo nel primo gruppo, in ordine di arrivo */
  autoAdd: boolean;
  /** cosa mostrano le uscite senza video (vuote, fuori onda, camera spenta) */
  emptyBg: "transparent" | "black";
  /** nomi con precedenza, in ordine: entrano davanti agli altri */
  priority: string[];
}

/** Uscita OBS (Fonte browser http://127.0.0.1:7878/slot/{id}); l'ordine è quello dell'elenco */
export interface Output {
  id: number;
  name: string;
  /** eccezione allo sfondo globale senza video; null = come Settings.emptyBg */
  bg: "transparent" | "black" | null;
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
  outputs: Output[];
  slots: Record<string, SlotAssign>;
  viewers: Record<string, number>;
  settings: Settings;
  groups: GroupView[];
  obs: ObsConfig;
  /** toggle "ospiti a piena risoluzione": on = richiesto; status = cosa vede lo studio */
  /** status: off | on | missing | manual | created (ricarica lo studio) | error:<motivo> | "" (in corso) */
  fullRes: { on: boolean; status: string; layout: string };
}
