/**
 * Client obs-websocket v5 + esecutore delle azioni OBS dei gruppi.
 *
 * Regole (come guestroom.live):
 * - agisce solo con "Pilota OBS" attivo e solo se la scena di programma è una delle scene
 *   del gruppo (quelle nominate dalle azioni, i contenitori degli elementi, le scene extra);
 * - agisce quando cambia il numero di partecipanti, oppure quando OBS arriva su una scena
 *   del gruppo mentre un cambio è in attesa; i passaggi manuali tra le scene del gruppo
 *   restano intatti fino al cambio successivo;
 * - un gruppo può avere altri set di scene (es. "Cam 1/2/3" e "Reaction 1/2/3"): agisce il set
 *   che contiene la scena di programma, quindi su "Reaction 2" un terzo partecipante porta a
 *   "Reaction 3", mai alle scene camera; passando a una scena di un altro set, lo stato di quel
 *   set per il numero attuale si applica una volta;
 * - non tocca mai streaming, registrazione, replay buffer o virtual cam.
 *
 * Sottopancia in fonti Testo (Sottopancia: "In fonti Testo di OBS"): per ogni uscita scrive nome e
 * seconda riga nelle fonti "<uscita> nome" e "<uscita> titolo" (SetInputSettings), le svuota quando
 * l'uscita si svuota e può crearle nella scena corrente.
 */
import { reactive, watch } from "vue";
import { state, type GroupAction, type GroupView } from "./store";

export const SEP = " › ";

/** "Scena › Fonte" (accetta anche "Scena > Fonte") */
export const parseItem = (ref: string): [string, string] | null => {
  const m = /^(.*?)\s*(?:›|>)\s*(.+)$/.exec(ref.trim());
  return m && m[1] ? [m[1].trim(), m[2].trim()] : null;
};

export type GroupObsStatus =
  | { kind: "off" } // Pilota OBS spento
  | { kind: "offline" }
  | { kind: "none" } // il gruppo non ha azioni OBS
  | { kind: "away"; scene: string } // OBS non è su una scena del gruppo
  | { kind: "missing"; items: string[]; set?: string }
  | { kind: "synced"; set?: string };

export const obs = reactive({
  status: "idle" as "idle" | "connecting" | "connected" | "auth" | "error",
  error: "",
  programScene: "",
  scenes: [] as string[],
  /** suggerimenti per gli elementi: "Contenitore › Fonte" */
  items: [] as string[],
  groupStatus: {} as Record<number, GroupObsStatus>,
  /** sottopancia in fonti Testo: fonti che mancano in OBS, per uscita */
  ltMissing: {} as Record<number, string[]>,
});

// ------------------------------------------------------------------ client
let ws: WebSocket | null = null;
let reqId = 0;
const pending = new Map<string, { ok: (d: any) => void; ko: (e: Error) => void }>();
let retry = 0;
let wanted = "";

const b64sha256 = async (s: string) => {
  const d = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(s));
  return btoa(String.fromCharCode(...new Uint8Array(d)));
};

export function request<T = any>(requestType: string, requestData?: Record<string, unknown>): Promise<T> {
  if (!ws || obs.status !== "connected") return Promise.reject(new Error("OBS non connesso"));
  const requestId = String(++reqId);
  return new Promise((ok, ko) => {
    pending.set(requestId, { ok, ko });
    ws!.send(JSON.stringify({ op: 6, d: { requestType, requestId, requestData } }));
  });
}

function connect() {
  clearTimeout(retry);
  ws?.close();
  ws = null;
  const { url, password } = state.obs;
  if (!url) return void (obs.status = "idle");
  obs.status = "connecting";
  const sock = (ws = new WebSocket(url));
  sock.onmessage = async (ev) => {
    const { op, d } = JSON.parse(ev.data);
    if (op === 0) {
      // Hello
      let authentication: string | undefined;
      if (d.authentication) {
        const secret = await b64sha256(password + d.authentication.salt);
        authentication = await b64sha256(secret + d.authentication.challenge);
      }
      // eventi: General(1) | Scenes(4) | Inputs(8) | SceneItems(128)
      sock.send(JSON.stringify({ op: 1, d: { rpcVersion: 1, authentication, eventSubscriptions: 1 | 4 | 8 | 128 } }));
    } else if (op === 2) {
      obs.status = "connected";
      obs.error = "";
      ltSent.clear();
      await refreshCatalog();
      reconcile();
      void syncLowerThirds();
    } else if (op === 5) {
      if (d.eventType === "CurrentProgramSceneChanged") {
        obs.programScene = d.eventData.sceneName;
        reconcile();
      } else if (/^Scene(Item)?(Created|Removed|NameChanged)$/.test(d.eventType)) {
        void refreshCatalog();
      } else if (/^Input(Created|Removed|NameChanged)$/.test(d.eventType)) {
        ltSent.clear(); // una fonte Testo creata o rinominata: riscrivi tutto
        void syncLowerThirds();
      }
    } else if (op === 7) {
      const p = pending.get(d.requestId);
      pending.delete(d.requestId);
      if (!p) return;
      if (d.requestStatus.result) p.ok(d.responseData ?? {});
      else p.ko(new Error(d.requestStatus.comment || `errore ${d.requestStatus.code}`));
    }
  };
  sock.onclose = (ev) => {
    if (ws !== sock) return;
    pending.forEach((p) => p.ko(new Error("OBS disconnesso")));
    pending.clear();
    if (ev.code === 4009) {
      obs.status = "auth";
      obs.error = "Password obs-websocket errata";
      return; // non riprovare finché la configurazione non cambia
    }
    obs.status = "error";
    obs.error = "OBS non raggiungibile";
    retry = window.setTimeout(connect, 3000);
  };
}

function disconnect() {
  clearTimeout(retry);
  const sock = ws;
  ws = null; // onclose di questo socket non riprova (ws !== sock)
  sock?.close();
  pending.forEach((p) => p.ko(new Error("OBS disconnesso")));
  pending.clear();
  obs.status = "idle";
  obs.error = "";
}

/** scene ed elementi di OBS (solo nomi), per i suggerimenti dell'editor */
export async function refreshCatalog() {
  try {
    const { scenes, currentProgramSceneName } = await request("GetSceneList");
    obs.programScene = currentProgramSceneName;
    const names: string[] = scenes.map((s: any) => s.sceneName).reverse(); // ordine come in OBS
    const items: string[] = [];
    const seenGroups = new Set<string>();
    for (const scene of names) {
      const { sceneItems } = await request("GetSceneItemList", { sceneName: scene });
      for (const it of sceneItems) {
        items.push(scene + SEP + it.sourceName);
        if (it.isGroup && !seenGroups.has(it.sourceName)) {
          seenGroups.add(it.sourceName);
          const g = await request("GetGroupSceneItemList", { sceneName: it.sourceName }).catch(() => null);
          g?.sceneItems.forEach((c: any) => items.push(it.sourceName + SEP + c.sourceName));
        }
      }
    }
    obs.scenes = names;
    obs.items = [...new Set(items)];
  } catch {
    /* OBS chiuso a metà: ci pensa la riconnessione */
  }
}

// ------------------------------------------------------------- esecutore
/** Un set di scene del gruppo: il principale (indice 0) o uno di `sets` (indice i = sets[i - 1]). */
export interface SceneSet {
  name: string;
  actions: Record<string, GroupAction>;
  extraScenes: string[];
}

export const MAIN_SET = "Principale";

/** i set di scene di un gruppo: il principale per primo */
export const groupSets = (g: Pick<GroupView, "actions" | "extraScenes" | "sets">): SceneSet[] => [
  { name: MAIN_SET, actions: g.actions, extraScenes: g.extraScenes },
  ...(g.sets ?? []),
];

/** ultimo stato applicato per gruppo: il set e cosa voleva (numero e configurazione) */
const applied = new Map<number, { set: number; key: string }>();

/** le scene su cui agisce un set: quelle nominate dalle azioni, i contenitori degli elementi, le extra */
export const setScenes = (g: Pick<SceneSet, "actions" | "extraScenes">): Set<string> => {
  const s = new Set<string>(g.extraScenes);
  for (const a of Object.values(g.actions)) {
    if (a.scene) s.add(a.scene);
    for (const ref of a.items) {
      const p = parseItem(ref);
      if (p) s.add(p[0]); // se è un gruppo OBS non sarà mai la scena di programma: innocuo
    }
  }
  return s;
};

/** le scene che più set userebbero (rifiutate: la scena di programma dice quale set agisce) */
export const sharedScenes = (sets: Pick<SceneSet, "actions" | "extraScenes">[]): string[] => {
  const seen = new Set<string>();
  const shared = new Set<string>();
  for (const set of sets) for (const sc of setScenes(set)) (seen.has(sc) ? shared : seen).add(sc);
  return [...shared];
};

async function findItemId(container: string, source: string): Promise<number | null> {
  const list =
    (await request("GetSceneItemList", { sceneName: container }).catch(() => null)) ??
    (await request("GetGroupSceneItemList", { sceneName: container }).catch(() => null));
  const it = list?.sceneItems.find((i: any) => i.sourceName === source);
  return it ? it.sceneItemId : null;
}

async function apply(g: GroupView, index: number, set: SceneSet, key: string) {
  const action = set.actions[String(g.count)] ?? { items: [] };
  const all = new Set(Object.values(set.actions).flatMap((a) => a.items));
  const missing: string[] = [];
  // prima gli elementi, poi la scena: niente fotogrammi con il layout sbagliato
  for (const ref of all) {
    const p = parseItem(ref);
    const id = p && (await findItemId(p[0], p[1]));
    if (!p || id == null) {
      missing.push(ref);
      continue;
    }
    await request("SetSceneItemEnabled", {
      sceneName: p[0],
      sceneItemId: id,
      sceneItemEnabled: action.items.includes(ref),
    }).catch(() => missing.push(ref));
  }
  if (action.scene && action.scene !== obs.programScene) {
    await request("SetCurrentProgramScene", { sceneName: action.scene }).catch(() => missing.push(action.scene!));
  }
  applied.set(g.id, { set: index, key });
  const name = g.sets?.length ? set.name : undefined;
  obs.groupStatus[g.id] = missing.length ? { kind: "missing", items: missing, set: name } : { kind: "synced", set: name };
}

let running = Promise.resolve();
export function reconcile() {
  running = running.then(async () => {
    for (const g of state.groups) {
      const sets = groupSets(g);
      const scenes = sets.map(setScenes);
      const index = scenes.findIndex((s) => s.has(obs.programScene));
      const key = index < 0 ? "" : JSON.stringify([g.count, sets[index].actions, sets[index].extraScenes]);
      const last = applied.get(g.id);
      if (scenes.every((s) => !s.size)) obs.groupStatus[g.id] = { kind: "none" };
      else if (!state.obs.drive) obs.groupStatus[g.id] = { kind: "off" };
      else if (obs.status !== "connected") obs.groupStatus[g.id] = { kind: "offline" };
      else if (index < 0) obs.groupStatus[g.id] = { kind: "away", scene: obs.programScene };
      else if (last?.set !== index || last.key !== key) await apply(g, index, sets[index], key).catch(() => (obs.groupStatus[g.id] = { kind: "offline" }));
      else if (obs.groupStatus[g.id]?.kind !== "missing") obs.groupStatus[g.id] = { kind: "synced", set: g.sets?.length ? sets[index].name : undefined };
    }
  });
  return running;
}

/** riapplica subito un gruppo (dopo averlo modificato) */
export function forceApply(id: number) {
  applied.delete(id);
  reconcile();
}

// ------------------------------------------- sottopancia in fonti Testo
/** nomi delle due fonti Testo di un'uscita */
export const ltInputs = (output: string) => ({ name: `${output} nome`, title: `${output} titolo` });
/** serve OBS: "Pilota OBS" attivo oppure sottopancia nelle fonti Testo */
export const obsWanted = () => state.obs.drive || state.settings.lowerThirds === "obs";

/** ultimo testo scritto per fonte (svuotato a ogni connessione) */
const ltSent = new Map<string, string>();
let ltRunning = Promise.resolve();

export function syncLowerThirds() {
  ltRunning = ltRunning.then(async () => {
    if (obs.status !== "connected") return void (obs.ltMissing = {});
    const on = state.settings.lowerThirds === "obs";
    const missing: Record<number, string[]> = {};
    for (const o of state.outputs) {
      const lt = on && o.lt ? state.lowerThirds[String(o.id)] : undefined;
      const names = ltInputs(o.name);
      for (const [input, text] of [[names.name, lt?.name ?? ""], [names.title, lt?.title ?? ""]] as const) {
        // sottopancia spenti: svuota solo le fonti che abbiamo scritto noi
        if (!on && !ltSent.get(input)) continue;
        if (ltSent.get(input) === text) continue;
        try {
          await request("SetInputSettings", { inputName: input, inputSettings: { text }, overlay: true });
          ltSent.set(input, text);
        } catch {
          ltSent.delete(input);
          if (on && o.lt) (missing[o.id] ??= []).push(input);
        }
      }
    }
    obs.ltMissing = missing;
  });
  return ltRunning;
}

/** Crea in OBS le fonti Testo che mancano, in basso a sinistra della scena corrente (anteprima in Modalità studio). */
export async function createLtInputs(): Promise<number> {
  const { inputKinds } = await request<{ inputKinds: string[] }>("GetInputKindList");
  const kind = inputKinds.find((k) => k.startsWith("text_gdiplus")) ?? inputKinds.find((k) => k.startsWith("text_ft2_source"));
  if (!kind) throw new Error("OBS non ha un tipo di fonte Testo");
  const scene =
    (await request("GetCurrentPreviewScene").then((r) => r.currentPreviewSceneName as string).catch(() => "")) ||
    (await request("GetCurrentProgramScene")).currentProgramSceneName;
  const { baseHeight } = await request("GetVideoSettings");
  let made = 0;
  for (const [id, inputs] of Object.entries(obs.ltMissing)) {
    const out = state.outputs.find((o) => o.id === Number(id));
    if (!out) continue;
    const names = ltInputs(out.name);
    for (const input of inputs) {
      const isName = input === names.name;
      const size = Math.round(baseHeight * (isName ? 0.044 : 0.026));
      const { sceneItemId } = await request("CreateInput", {
        sceneName: scene,
        inputName: input,
        inputKind: kind,
        inputSettings: { text: "", font: { face: "Segoe UI", size, style: isName ? "Bold" : "Regular", flags: isName ? 1 : 0 } },
        sceneItemEnabled: true,
      });
      await request("SetSceneItemTransform", {
        sceneName: scene,
        sceneItemId,
        sceneItemTransform: { positionX: Math.round(baseHeight * 0.07), positionY: Math.round(baseHeight * (isName ? 0.84 : 0.895)) },
      }).catch(() => {});
      made++;
    }
  }
  ltSent.clear();
  await syncLowerThirds();
  return made;
}

export function startObs() {
  // collegati a OBS solo se serve ("Pilota OBS" o sottopancia in fonti Testo);
  // riconnetti se cambiano indirizzo o password
  watch(
    () => [obsWanted(), state.obs.url, state.obs.password] as const,
    ([on, url, pw]) => {
      const key = on ? url + "\n" + pw : "";
      if (key === wanted) return;
      wanted = key;
      if (on) connect();
      // prima svuota le fonti Testo scritte da noi (sottopancia appena spenti), poi chiudi
      else void syncLowerThirds().finally(() => !wanted && disconnect());
    },
    { immediate: true },
  );
  // sottopancia nelle fonti Testo: riscrivi quando cambia chi è sulle uscite, i testi o i nomi
  watch(
    () => JSON.stringify([state.settings.lowerThirds, state.lowerThirds, state.outputs.map((o) => [o.id, o.name, o.lt]), obs.status]),
    () => void syncLowerThirds(),
  );
  // riconcilia quando cambiano conteggi, azioni o Pilota OBS
  watch(
    () => JSON.stringify([state.obs.drive, state.groups.map((g) => [g.id, g.count, g.actions, g.extraScenes, g.sets])]),
    () => reconcile(),
  );
}
