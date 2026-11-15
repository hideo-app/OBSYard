/**
 * Client obs-websocket v5 + esecutore delle azioni OBS dei gruppi.
 *
 * Regole (come guestroom.live):
 * - agisce solo con "Pilota OBS" attivo e solo se la scena di programma è una delle scene
 *   del gruppo (quelle nominate dalle azioni, i contenitori degli elementi, le scene extra);
 * - agisce quando cambia il numero di partecipanti, oppure quando OBS arriva su una scena
 *   del gruppo mentre un cambio è in attesa; i passaggi manuali tra le scene del gruppo
 *   restano intatti fino al cambio successivo;
 * - non tocca mai streaming, registrazione, replay buffer o virtual cam.
 */
import { reactive, watch } from "vue";
import { state, type GroupView } from "./store";

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
  | { kind: "missing"; items: string[] }
  | { kind: "synced" };

export const obs = reactive({
  status: "idle" as "idle" | "connecting" | "connected" | "auth" | "error",
  error: "",
  programScene: "",
  scenes: [] as string[],
  /** suggerimenti per gli elementi: "Contenitore › Fonte" */
  items: [] as string[],
  groupStatus: {} as Record<number, GroupObsStatus>,
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
      // eventi: General(1) | Scenes(4) | SceneItems(128)
      sock.send(JSON.stringify({ op: 1, d: { rpcVersion: 1, authentication, eventSubscriptions: 1 | 4 | 128 } }));
    } else if (op === 2) {
      obs.status = "connected";
      obs.error = "";
      await refreshCatalog();
      reconcile();
    } else if (op === 5) {
      if (d.eventType === "CurrentProgramSceneChanged") {
        obs.programScene = d.eventData.sceneName;
        reconcile();
      } else if (/^Scene(Item)?(Created|Removed|NameChanged)$/.test(d.eventType)) {
        void refreshCatalog();
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
/** ultimo numero di partecipanti applicato per gruppo */
const applied = new Map<number, number>();

export const groupScenes = (g: GroupView): Set<string> => {
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

async function findItemId(container: string, source: string): Promise<number | null> {
  const list =
    (await request("GetSceneItemList", { sceneName: container }).catch(() => null)) ??
    (await request("GetGroupSceneItemList", { sceneName: container }).catch(() => null));
  const it = list?.sceneItems.find((i: any) => i.sourceName === source);
  return it ? it.sceneItemId : null;
}

async function apply(g: GroupView) {
  const action = g.actions[String(g.count)] ?? { items: [] };
  const all = new Set(Object.values(g.actions).flatMap((a) => a.items));
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
  applied.set(g.id, g.count);
  obs.groupStatus[g.id] = missing.length ? { kind: "missing", items: missing } : { kind: "synced" };
}

let running = Promise.resolve();
export function reconcile() {
  running = running.then(async () => {
    for (const g of state.groups) {
      const scenes = groupScenes(g);
      if (!scenes.size) obs.groupStatus[g.id] = { kind: "none" };
      else if (!state.obs.drive) obs.groupStatus[g.id] = { kind: "off" };
      else if (obs.status !== "connected") obs.groupStatus[g.id] = { kind: "offline" };
      else if (!scenes.has(obs.programScene)) obs.groupStatus[g.id] = { kind: "away", scene: obs.programScene };
      else if (applied.get(g.id) !== g.count) await apply(g).catch(() => (obs.groupStatus[g.id] = { kind: "offline" }));
      else if (obs.groupStatus[g.id]?.kind !== "missing") obs.groupStatus[g.id] = { kind: "synced" };
    }
  });
  return running;
}

/** riapplica subito un gruppo (dopo averlo modificato) */
export function forceApply(id: number) {
  applied.delete(id);
  reconcile();
}

export function startObs() {
  // riconnetti quando cambiano indirizzo o password
  // collegati a OBS solo con "Pilota OBS" attivo; riconnetti se cambiano indirizzo o password
  watch(
    () => [state.obs.drive, state.obs.url, state.obs.password] as const,
    ([drive, url, pw]) => {
      const key = drive ? url + "\n" + pw : "";
      if (key === wanted) return;
      wanted = key;
      if (drive) connect();
      else disconnect();
    },
    { immediate: true },
  );
  // riconcilia quando cambiano conteggi, azioni o Pilota OBS
  watch(
    () => JSON.stringify([state.obs.drive, state.groups.map((g) => [g.id, g.count, g.actions, g.extraScenes])]),
    () => reconcile(),
  );
}
