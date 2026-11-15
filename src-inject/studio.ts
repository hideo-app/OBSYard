/**
 * Iniettato in StreamYard PRIMA degli script della pagina (initialization_script).
 *
 * 1. Intercetta le tracce: RTCPeerConnection "track" (ospiti remoti),
 *    getUserMedia (camera/mic locali), getDisplayMedia (condivisioni schermo).
 * 2. Per ogni fonte browser OBS connessa (viewer) crea una RTCPeerConnection
 *    di loopback sendonly e ci collega la traccia assegnata allo slot.
 *    Cambiare sorgente = sender.replaceTrack(): nessuna rinegoziazione, nessuno scatto.
 *
 * Signaling: pagina -> Rust via IPC Tauri (invoke), Rust -> pagina via eval(recv(...)).
 */
import type { Output, Settings, SlotAssign, TrackInfo, TrackOrigin } from "./types";

(() => {
  const W = window as any;
  if (window.top !== window || W.__obsyard) return;

  const OrigPC: typeof RTCPeerConnection = window.RTCPeerConnection;
  const log = (...a: unknown[]) => console.debug("[obsyard]", ...a);
  /** ponte WebView2 (buffer condivisi verso il plugin OBS), preso prima che la pagina giri */
  const webview: any = W.chrome?.webview;

  const invoke = (cmd: string, args?: Record<string, unknown>): Promise<unknown> => {
    const t = W.__TAURI_INTERNALS__;
    if (!t) return Promise.reject(new Error("IPC Tauri non disponibile"));
    return t.invoke(cmd, args);
  };

  // ------------------------------------------------------------------ stato
  interface Entry {
    track: MediaStreamTrack;
    origin: TrackOrigin;
    label: string;
    streamIds: string[];
    /** id dello stream StreamYard (dal protocollo), se noto */
    syStream?: string;
    /** ricevitore WebRTC (tracce remote): serve a capire se arrivano ancora frame */
    receiver?: RTCRtpReceiver;
    frames?: number;
    lastFrameAt?: number;
    /** nessun frame da un po' (camera spenta, ospite congelato) */
    stalled?: boolean;
    /** Discord: ssrc ricevuto (dalle statistiche) e a chi appartiene (dalla connessione vocale) */
    ssrc?: number;
    dcUser?: string;
    dcKind?: "video" | "screen";
    w?: number;
    h?: number;
  }
  /** chiave = id nostro univoco: Janus chiama "janus0/janus1" le tracce di OGNI ospite */
  const tracks = new Map<string, Entry>();
  const uids = new WeakMap<MediaStreamTrack, string>();
  let seq = 0;
  const uidOf = (t: MediaStreamTrack | null | undefined) => (t ? uids.get(t) : undefined);

  // ------------------------------------------ stato StreamYard (protocollo)
  // streamUpdated: privacy "public" = in onda, "moderators" = backstage; clientUpdated: nomi.
  const syStreams = new Map<string, { clientId: string; privacy: string; kind: string; hasVideo: boolean }>();
  const syNames = new Map<string, string>();
  /** riga "o=" dell'SDP dell'offerta Janus -> streamId (per legare la PeerConnection allo stream) */
  const offerStream = new Map<string, string>();
  const pcStream = new WeakMap<RTCPeerConnection, string>();
  /** id traccia locale -> streamId (dalle offerte che pubblichiamo) */
  const localStream = new Map<string, string>();
  /** connectionId -> id tracce locali (dall'offerta che pubblichiamo) */
  const connTracks = new Map<string, string[]>();
  let ownClientId: string | undefined;
  /** workspace StreamYard (per l'API dei layout) e massimo di camere in onda del piano */
  let workspaceId: string | undefined;
  let maxPublicCams = 10;
  const setName = (clientId: string | undefined, name: string | undefined) => {
    if (!clientId || !name || syNames.get(clientId) === name) return false;
    syNames.set(clientId, name);
    return true;
  };
  const originLine = (sdp: string) => /^o=.*$/m.exec(sdp)?.[0] ?? "";

  const syInfo = (e: Entry) => {
    const sid = e.syStream ?? localStream.get(e.track.id);
    if (sid) return syStreams.get(sid);
    // Camera/mic locali: StreamYard pubblica una copia elaborata (filtri colore), con un altro id.
    // Noi inoltriamo l'originale: lo leghiamo al nostro stream dello stesso tipo.
    if (e.origin !== "remote" && ownClientId) {
      const kind = e.origin === "screen" ? "screen" : "webcam";
      for (const s of syStreams.values()) if (s.clientId === ownClientId && s.kind === kind) return s;
    }
    return undefined;
  };
  // ------------------------------------------ Discord (connessione vocale)
  // La connessione vocale (wss://*.discord.media, JSON) dice a chi appartiene ogni ssrc:
  //   op 12 "video": { user_id, audio_ssrc, video_ssrc, streams: [{ type: "video"|"screen", ssrc, rtx_ssrc, active }] }
  //   op 5 "speaking": { user_id, ssrc }   op 13 "client disconnect": { user_id }
  // Le connessioni "Go Live" (schermo condiviso) sono connessioni vocali a parte.
  const IS_DISCORD = /(^|\.)discord\.com$/.test(location.hostname);
  const dcSsrc = new Map<number, { user: string; kind: "video" | "screen" }>();
  /** "utente:tipo" -> video attivo */
  const dcActive = new Map<string, boolean>();
  const dcStreamWs = new WeakSet<WebSocket>();
  const dcNames = new Map<string, string>();
  const dcKey = (e: Entry) => `${e.dcUser}:${e.dcKind}`;
  /** PeerConnection aperte dalla pagina (Discord: chiamata e Go Live) */
  const allPcs = new Set<RTCPeerConnection>();
  /** tracce locali che stiamo davvero inviando nella chiamata (senders delle PeerConnection) */
  const dcSent = new Set<MediaStreamTrack>();
  /** il nostro id utente (dall'identificazione della connessione vocale, op 0) */
  let dcSelf: string | undefined;
  /**
   * Discord scrive nella sua SDP lo stream di ogni traccia ricevuta come "<id utente>-<ssrc>"
   * (a=msid:906910029519683625-38789 …): basta questo per sapere di chi è, senza aspettare i pacchetti.
   */
  const dcFromStreams = (e: Entry): { user: string; ssrc: number } | undefined => {
    for (const id of e.streamIds) {
      const m = /^(\d{15,22})-(\d+)$/.exec(id);
      if (m) return { user: m[1], ssrc: Number(m[2]) };
    }
    return undefined;
  };

  /** undefined = non sappiamo (es. pagina di test): la consideriamo visibile */
  const onAir = (e: Entry): boolean | undefined => {
    if (IS_DISCORD && e.origin === "remote") {
      // tracce non legate a nessuno: Discord prepara connessioni vuote in anticipo
      if (!e.dcUser) return false;
      return e.track.kind === "audio" || dcActive.get(dcKey(e)) !== false;
    }
    // Discord apre il microfono più volte (prove, elaborazione WebAudio): conta solo quello inviato
    if (IS_DISCORD) return dcSent.has(e.track);
    const s = syInfo(e);
    return s ? s.privacy === "public" : undefined;
  };
  const labelOf = (e: Entry) => {
    if (IS_DISCORD && e.origin !== "remote") {
      const me = (dcSelf && dcNames.get(dcSelf)) || "Tu";
      if (e.origin === "screen") return `${me} (schermo)`;
      return e.track.kind === "video" ? me : `${me} (microfono)`;
    }
    if (IS_DISCORD && e.dcUser) {
      const name = dcNames.get(e.dcUser) ?? `Utente …${e.dcUser.slice(-4)}`;
      return e.dcKind === "screen" ? `${name} (schermo)` : name;
    }
    const s = syInfo(e);
    const name = s && syNames.get(s.clientId);
    if (!name) return e.label;
    return s!.kind === "webcam" ? name : `${name} (${s!.kind === "screen" ? "schermo" : s!.kind})`;
  };
  /** tracce terminate: servono per ri-agganciare automaticamente lo stesso ospite se riconnette */
  const ended = new Map<string, { kind: string; origin: TrackOrigin; label: string }>();
  let slots: Record<string, SlotAssign> = {};
  let settings: Settings = { codec: "H264", maxBitrateKbps: 12000, maxFps: 60, fullResLayout: "OBSYard", autoAdd: false, autoAddAudio: false, platform: "streamyard", emptyBg: "transparent", priority: [], lowerThirds: "off", thumbFps: 0.5 };
  let outputs: Output[] = [];
  let thumbsOn = true;
  const counters: Record<TrackOrigin, number> = { remote: 0, camera: 0, screen: 0 };

  // -------------------------------------------------------- pubblicazione
  let publishTimer = 0;
  /** schermo condiviso (nostro o di un ospite), non una webcam */
  const isScreen = (e: Entry) => {
    if (e.origin === "screen") return true;
    if (IS_DISCORD) return e.dcKind === "screen"; // Go Live

    const s = syInfo(e);
    return !!s && s.kind !== "webcam";
  };
  const publishTracks = () => {
    if (publishTimer) return;
    publishTimer = window.setTimeout(() => {
      publishTimer = 0;
      // solo le sorgenti in onda (o di cui StreamYard non ci dice nulla)
      const list: TrackInfo[] = [...tracks]
        .filter(([, e]) => onAir(e) !== false)
        .map(([id, e]) => ({
          id,
          kind: e.track.kind as "video" | "audio",
          // gli schermi condivisi dagli ospiti arrivano come tracce remote: StreamYard ci dice che
          // sono "screen" (o altro che non è una webcam) -> li pubblichiamo come schermo, così
          // l'aggiunta automatica non li considera e si assegnano sempre a mano
          origin: isScreen(e) ? "screen" : e.origin,
          label: labelOf(e),
          streamIds: e.syStream ? [e.syStream] : e.streamIds,
          muted: e.track.muted,
          w: e.w,
          h: e.h,
        }));
      // chi ha solo l'audio (camera spenta, solo voce): una sorgente "solo audio" con il suo nome,
      // da mettere su un'uscita (nessuna immagine, la sua voce) o nei gruppi
      const visible = [...tracks].filter(([, e]) => onAir(e) !== false);
      const paired = new Set<MediaStreamTrack>();
      for (const [, e] of visible) {
        if (e.track.kind !== "video") continue;
        const a = pairedAudio(e);
        if (a) paired.add(a);
      }
      for (const [id, e] of visible) {
        if (e.track.kind !== "audio" || paired.has(e.track) || isScreen(e)) continue;
        list.push({
          id: AUDIO_ONLY + id,
          kind: "video",
          origin: e.origin,
          label: labelOf(e).replace(/ \(microfono\)$/, ""),
          streamIds: e.streamIds,
          muted: false,
          audioOnly: true,
        });
      }
      invoke("pub_tracks", { tracks: list }).catch(() => {});
    }, 80);
  };

  const register = (
    track: MediaStreamTrack,
    origin: TrackOrigin,
    streams: readonly MediaStream[] = [],
    syStream?: string,
    receiver?: RTCRtpReceiver,
  ) => {
    if (uids.has(track) || track.readyState === "ended") return;
    const n = ++counters[origin];
    const uid = `${origin[0]}${++seq}-${track.id}`;
    uids.set(track, uid);
    const base = origin === "remote" ? "Ospite" : origin === "camera" ? (track.kind === "video" ? "Camera locale" : "Microfono locale") : "Schermo";
    tracks.set(uid, { track, origin, label: `${base} ${n}`, streamIds: streams.map((s) => s.id), syStream, receiver });
    const onEnd = () => {
      const e = tracks.get(uid);
      if (!e) return;
      tracks.delete(uid);
      ended.set(uid, { kind: track.kind, origin: e.origin, label: labelOf(e) });
      refreshAll();
      publishTracks();
    };
    track.addEventListener("ended", onEnd);
    // senza frame (camera spenta, ospite congelato): la fonte OBS nasconde l'ultimo fotogramma
    const onMute = () => {
      publishTracks();
      refreshAll();
    };
    track.addEventListener("mute", onMute);
    track.addEventListener("unmute", onMute);
    log("traccia", origin, track.kind, uid, syStream ?? "");
    refreshAll();
    publishTracks();
  };

  // ------------------------------------------------------------ hook WebRTC
  // Proxy (non sottoclasse) così newTarget, instanceof e sottoclassi della pagina restano intatti.
  const HookedPC = new Proxy(OrigPC, {
    construct(target, args, newTarget) {
      const pc: RTCPeerConnection = Reflect.construct(target, args, newTarget);
      allPcs.add(pc);
      pc.addEventListener("connectionstatechange", () => pc.connectionState === "closed" && allPcs.delete(pc));
      pc.addEventListener("track", (e: RTCTrackEvent) => register(e.track, "remote", e.streams, pcStream.get(pc), e.receiver));
      return pc;
    },
  });
  // lega la PeerConnection allo stream StreamYard: l'offerta Janus arriva col suo streamId
  const origSRD = OrigPC.prototype.setRemoteDescription;
  OrigPC.prototype.setRemoteDescription = function (this: RTCPeerConnection, d?: RTCSessionDescriptionInit) {
    const sid = d?.sdp && offerStream.get(originLine(d.sdp));
    if (sid) pcStream.set(this, sid);
    if (IS_DISCORD && d?.type === "answer" && d.sdp && allPcs.has(this)) {
      try {
        const sdp = dcAddAv1(d.sdp);
        if (sdp !== d.sdp) arguments[0] = { type: d.type, sdp };
      } catch (e) {
        log("discord: AV1 in ricezione", e);
      }
    }
    return origSRD.apply(this, arguments as any);
  } as any;
  W.RTCPeerConnection = HookedPC;
  if (W.webkitRTCPeerConnection) W.webkitRTCPeerConnection = HookedPC;

  const md = navigator.mediaDevices;
  if (md) {
    const gum = md.getUserMedia?.bind(md);
    if (gum)
      md.getUserMedia = async (c?: MediaStreamConstraints) => {
        const s = await gum(c);
        s.getTracks().forEach((t) => register(t, "camera", [s]));
        return s;
      };
    const gdm = md.getDisplayMedia?.bind(md);
    if (gdm)
      md.getDisplayMedia = async (c?: DisplayMediaStreamOptions) => {
        const s = await gdm(c);
        s.getTracks().forEach((t) => register(t, "screen", [s]));
        return s;
      };
  }

  // ------------------------------------------- WebSocket di StreamYard
  // Teniamo i socket aperti dalla pagina (servono per il layout "piena risoluzione").
  // Il tap registra i messaggi solo se attivato da debug: window.__obsyardTap.on = true
  const sockets: WebSocket[] = [];
  // debug: localStorage.obsyardTap = "1" registra fin dal caricamento della pagina
  let tapOn = false;
  try {
    tapOn = localStorage.getItem("obsyardTap") === "1";
  } catch {}
  const tap = { on: tapOn, log: [] as { dir: "out" | "in" | "http"; t: number; url: string; data: string }[] };
  const record = (dir: "out" | "in", ws: WebSocket, data: unknown) => {
    // Discord: la connessione vocale si registra sempre (pochi messaggi piccoli), perché Discord
    // toglie localStorage alla pagina e il tap non si può accendere prima di entrare in chiamata
    if (typeof data !== "string" || !(tap.on || (IS_DISCORD && /discord\.media/.test(ws.url)))) return;
    tap.log.push({ dir, t: Date.now(), url: ws.url, data: data.length > 8000 ? data.slice(0, 8000) + "…" : data });
    if (tap.log.length > 2000) tap.log.splice(0, 500);
  };
  /** Legge dal protocollo di StreamYard chi è in onda, i nomi e il legame traccia↔stream. */
  const syMessage = (dir: "out" | "in", data: unknown) => {
    if (typeof data !== "string" || !data.includes('"command"')) return;
    let m: any;
    try {
      m = JSON.parse(data).message;
    } catch {
      return;
    }
    const b = m?.body;
    if (!b) return;
    let changed = false;
    switch (m.command) {
      case "streamUpdated":
        if (b.streamId) {
          const prev = syStreams.get(b.streamId);
          const hasVideo = b.hasVideo !== false;
          syStreams.set(b.streamId, { clientId: b.clientId, privacy: b.privacy, kind: b.kind, hasVideo });
          changed = !prev || prev.privacy !== b.privacy || prev.kind !== b.kind || prev.hasVideo !== hasVideo;
        }
        break;
      case "streamRemoved":
      case "unpublish":
        if (b.streamId) changed = syStreams.delete(b.streamId);
        break;
      case "setPrivacy": // il nostro cambio arriva prima della conferma del server
        if (dir === "out" && b.streamId && syStreams.has(b.streamId)) {
          syStreams.get(b.streamId)!.privacy = b.privacy;
          changed = true;
        }
        break;
      case "joined": // il nostro client, appena entrati nello studio
        ownClientId = b.clientId ?? ownClientId;
        if (typeof b.maxPublicCams === "number" && b.maxPublicCams > 0) maxPublicCams = b.maxPublicCams;
        changed = setName(b.clientId, b.displayName);
        break;
      case "setWorkspaceId":
      case "workspaceIdUpdated":
        if (b.id) workspaceId = b.id;
        // appena entrati nello studio (anche dopo un ricaricamento): se il toggle è acceso, riapplica
        // quando i pulsanti dei layout sono comparsi
        if (fullRes.on) setTimeout(() => void applyFullRes(), 2500);
        break;
      case "clientList": // chi era già nello studio quando siamo entrati
        for (const c of b.clients ?? []) changed = setName(c.clientId, c.displayName) || changed;
        break;
      case "clientUpdated":
        changed = setName(b.clientId, b.displayName);
        break;
      case "setDisplayName": // il nostro nome, impostato dalla pagina
        if (dir === "out") changed = setName(ownClientId, b.displayName);
        break;
      case "offer":
        if (dir === "in" && b.jsep?.sdp && b.streamId) offerStream.set(originLine(b.jsep.sdp), b.streamId);
        // offerte che pubblichiamo noi: "a=msid:<stream> <idtraccia>"; lo streamId arriva a parte
        // (publishStream/configure con lo stesso connectionId)
        if (dir === "out" && b.jsep?.sdp && b.connectionId)
          connTracks.set(b.connectionId, [...b.jsep.sdp.matchAll(/^a=msid:\S+ (\S+)/gm)].map((x: RegExpMatchArray) => x[1]));
        break;
    }
    if (dir === "out" && b.connectionId && b.streamId && connTracks.has(b.connectionId)) {
      for (const t of connTracks.get(b.connectionId)!)
        if (localStream.get(t) !== b.streamId) {
          localStream.set(t, b.streamId);
          changed = true;
        }
    }
    if (changed) {
      publishTracks();
      refreshAll();
    }
  };

  /** Messaggi della connessione vocale di Discord: ssrc -> utente, video attivo. */
  const dcMessage = (ws: WebSocket, data: unknown) => {
    if (!IS_DISCORD || typeof data !== "string" || !/discord\.media/.test(ws.url)) return;
    let m: any;
    try {
      m = JSON.parse(data);
    } catch {
      return;
    }
    const d = m?.d;
    if (!d) return;
    let changed = false;
    const map = (ssrc: unknown, user: string, kind: "video" | "screen") => {
      if (typeof ssrc !== "number" || !ssrc) return;
      const cur = dcSsrc.get(ssrc);
      if (cur?.user !== user || cur?.kind !== kind) {
        dcSsrc.set(ssrc, { user, kind });
        changed = true;
      }
    };
    const setActive = (key: string, on: boolean) => {
      if (dcActive.get(key) !== on) {
        dcActive.set(key, on);
        changed = true;
      }
    };
    if (m.op === 12 && d.user_id) {
      const user = String(d.user_id);
      const streams: any[] = Array.isArray(d.streams) ? d.streams : [];
      if (streams.some((s) => s.type === "screen")) dcStreamWs.add(ws);
      const base = dcStreamWs.has(ws) ? "screen" : "video";
      map(d.audio_ssrc, user, base);
      map(d.video_ssrc, user, base);
      map(d.rtx_ssrc, user, base);
      for (const s of streams) {
        const kind = s.type === "screen" ? "screen" : base;
        map(s.ssrc, user, kind);
        map(s.rtx_ssrc, user, kind);
      }
      if (streams.length) setActive(`${user}:${streams[0].type === "screen" ? "screen" : base}`, streams.some((s) => s.active !== false && s.ssrc));
      else if ("video_ssrc" in d) setActive(`${user}:${base}`, !!d.video_ssrc);
    } else if (m.op === 5 && d.user_id) {
      map(d.ssrc, String(d.user_id), dcStreamWs.has(ws) ? "screen" : "video");
    } else if (m.op === 13 && d.user_id) {
      const user = String(d.user_id);
      setActive(`${user}:video`, false);
      setActive(`${user}:screen`, false);
    }
    if (changed) void dcResolve();
  };

  /** Nome di un utente Discord dalla pagina: l'URL dell'avatar contiene l'id utente. */
  /** un'etichetta che sembra un nome (non un pulsante o una sezione della pagina) */
  const okName = (a: string | null | undefined) => {
    const n = (a || "").replace(/,.*$/, "").trim();
    return n && n.length <= 40 && !/^(opzioni|options|altro|more|video|canali|channels)$|avatar|riquadro|tile|impostazioni|settings|stato|status/i.test(n) ? n : null;
  };
  const dcFindName = (id: string): string | null => {
    // riquadro della chiamata: data-selenium-video-tile="<id>", con dentro il nome visualizzato
    for (const tile of document.querySelectorAll(`[data-selenium-video-tile="${id}"]`)) {
      // prima il contenitore col nome (focusTarget), poi gli altri aria-label tranne i pulsanti
      const labelled = [tile, ...tile.querySelectorAll('[class*="focusTarget"][aria-label]'), ...tile.querySelectorAll("[aria-label]")];
      for (const el of labelled) {
        if (el.tagName === "BUTTON" || el.getAttribute("role") === "button") continue;
        const n = okName(el.getAttribute("aria-label"));
        if (n) return n;
      }
      const u = okName(tile.querySelector<HTMLElement>('[class*="username"]')?.innerText);
      if (u) return u;
    }
    const fromAvatar = dcNameFromAvatar(id);
    if (fromAvatar) return fromAvatar;
    // ultimo ripiego: il nome utente nel riquadro ("Riquadro chiamata, lorenzotafuro")
    for (const tile of document.querySelectorAll(`[data-selenium-video-tile="${id}"]`)) {
      for (const el of [tile, ...tile.querySelectorAll("[aria-label]")]) {
        const m = /^(?:riquadro chiamata|call tile),\s*(.+)$/i.exec(el.getAttribute("aria-label") || "");
        if (m && m[1].length <= 40) return m[1].trim();
      }
    }
    return null;
  };

  /** nome dall'avatar: elenco del canale vocale, pannello account, riquadri senza video */
  const dcNameFromAvatar = (id: string): string | null => {
    // con la camera accesa l'avatar è uno sfondo (style="background-image: url(…/avatars/<id>/…)")
    const avatars = [
      ...document.querySelectorAll<HTMLElement>(`img[src*="/avatars/${id}/"]`),
      ...document.querySelectorAll<HTMLElement>(`[style*="/avatars/${id}/"]`),
    ];
    for (const img of avatars as HTMLImageElement[]) {
      const own = (img.getAttribute("aria-label") || img.alt || "").replace(/,.*$/, "").trim();
      if (own && own.length <= 40 && !/avatar/i.test(own)) return own;
      // risaliamo dall'avatar: a ogni livello prima l'aria-label (riquadri senza video), poi la
      // prima riga di testo (elenco del canale vocale, pannello account). Ci fermiamo al bordo
      // di un elenco: più su ci sono etichette come "Canali" o il nome del server.
      let node: Element | null = img.parentElement;
      for (let depth = 0; node && depth < 8; depth++, node = node.parentElement) {
        if (/^(UL|OL|NAV|LI|SECTION|ASIDE)$/.test(node.tagName) || /^(group|list|listbox|navigation|tree)$/.test(node.getAttribute("role") || "")) break;
        const a = okName(node.getAttribute("aria-label"));
        if (a) return a;
        const line = ((node as HTMLElement).innerText || "")
          .split("\n")
          .map((x) => x.trim())
          .find((x) => x.length >= 2 && x.length <= 40);
        if (line) return line;
      }
    }
    return null;
  };

  /** Lega ogni traccia remota al suo utente Discord leggendo l'ssrc dalle statistiche. */
  let dcBusy = false;
  const dcResolve = async () => {
    if (!IS_DISCORD || dcBusy) return;
    dcBusy = true;
    let changed = false;
    try {
      // tracce locali inviate nella chiamata (camera, microfono elaborato, Go Live)
      const sent = new Set<MediaStreamTrack>();
      for (const pc of allPcs) for (const snd of pc.getSenders()) if (snd.track && snd.track.readyState === "live") sent.add(snd.track);
      if (sent.size !== dcSent.size || [...sent].some((t) => !dcSent.has(t))) {
        dcSent.clear();
        sent.forEach((t) => dcSent.add(t));
        changed = true;
      }
      // il microfono che Discord invia è una sua copia elaborata (WebAudio), mai passata da getUserMedia
      for (const t of sent) if (!uids.has(t)) register(t, t.label.startsWith("screen") ? "screen" : "camera");

      for (const [uid, e] of [...tracks]) {
        if (e.origin !== "remote") continue;
        let ssrc = dcFromStreams(e)?.ssrc;
        if (ssrc === undefined && e.receiver) {
          try {
            (await e.receiver.getStats()).forEach((s: any) => {
              if (s.type === "inbound-rtp" && typeof s.ssrc === "number") ssrc = s.ssrc;
            });
          } catch {
            continue;
          }
        }
        if (ssrc === undefined) continue;
        e.ssrc = ssrc;
        // la connessione vocale dice anche se è camera o schermo; lo stream dice almeno di chi è
        const fromStream = dcFromStreams(e);
        const who = dcSsrc.get(ssrc) ?? (fromStream && { user: fromStream.user, kind: e.dcKind ?? ("video" as const) });
        if (!who || (who.user === e.dcUser && who.kind === e.dcKind)) continue;
        if (e.dcUser && e.dcUser !== who.user) {
          // Discord ha riusato la connessione per un altro utente: per noi è una sorgente nuova
          tracks.delete(uid);
          ended.set(uid, { kind: e.track.kind, origin: e.origin, label: labelOf(e) });
          const fresh = `r${++seq}-${e.track.id}`;
          uids.set(e.track, fresh);
          tracks.set(fresh, e);
        }
        e.dcUser = who.user;
        e.dcKind = who.kind;
        changed = true;
      }
      // nomi: possono comparire nella pagina dopo
      const users = new Set<string>();
      for (const e of tracks.values()) if (e.dcUser) users.add(e.dcUser);
      if (dcSelf) users.add(dcSelf);
      for (const id of users) {
        const name = dcFindName(id);
        if (name && dcNames.get(id) !== name) {
          dcNames.set(id, name);
          changed = true;
        }
      }
    } finally {
      dcBusy = false;
    }
    if (changed) {
      publishTracks();
      refreshAll();
    }
  };
  if (IS_DISCORD) setInterval(dcResolve, 1000);

  // ---------------------------------- Discord: qualità richiesta (op 15)
  // Ogni client dice al server quanta qualità vuole per ogni video ("media sink wants", 0–100 per
  // ssrc, più "any" per gli altri); il client ufficiale la abbassa quando un riquadro è piccolo, e
  // chi invia adegua il suo encoder a quello che gli chiedono. Con "Piena risoluzione" riscriviamo
  // la richiesta: 100 per tutti. Il tetto resta quello di chi invia (camera 720p, Go Live secondo
  // le sue impostazioni e Nitro).
  const origSend = window.WebSocket.prototype.send;
  const dcWants = new Map<WebSocket, Record<string, number>>();
  const dcBoost = (want: Record<string, number>) =>
    Object.fromEntries(Object.entries(want).map(([k, v]) => [k, typeof v === "number" ? 100 : v]));
  /** rimanda l'ultima richiesta di ogni connessione vocale, con o senza qualità massima */
  const dcResendWants = () => {
    for (const [ws, want] of dcWants) {
      if (ws.readyState !== WebSocket.OPEN) {
        dcWants.delete(ws);
        continue;
      }
      origSend.call(ws, JSON.stringify({ op: 15, d: fullRes.on ? dcBoost(want) : want }));
    }
  };

  // ------------------------------------- Discord: ricevere anche AV1
  /** payload type di AV1 (e del suo rtx) nella nostra offerta locale */
  const dcAv1Pt = (): { pt: number; rtx: number | null } | null => {
    for (const pc of allPcs) {
      const sdp = pc.localDescription?.sdp ?? "";
      const pt = /a=rtpmap:(\d+) AV1\/90000/.exec(sdp)?.[1];
      if (!pt) continue;
      const rtx = sdp.split(/\r?\n/).map((l) => /^a=fmtp:(\d+) apt=(\d+)\s*$/.exec(l)).find((m) => m?.[2] === pt)?.[1];
      return { pt: Number(pt), rtx: rtx ? Number(rtx) : null };
    }
    return null;
  };
  /**
   * Discord costruisce da sé l'SDP di risposta con il solo codec della sessione (H.264): un video
   * che arriva in AV1 verrebbe scartato. Aggiungiamo AV1 a ogni sezione video, DOPO il codec di
   * Discord, così per inviare resta scelto il suo.
   */
  const dcAddAv1 = (sdp: string): string => {
    const av1 = dcAv1Pt();
    if (!av1 || /AV1\/90000/.test(sdp)) return sdp;
    const nl = sdp.includes("\r\n") ? "\r\n" : "\n";
    const out: string[] = [];
    let video = false;
    for (const line of sdp.split(nl)) {
      if (line.startsWith("m=")) {
        video = line.startsWith("m=video");
        out.push(video ? `${line} ${av1.pt}${av1.rtx ? " " + av1.rtx : ""}` : line);
        if (video) {
          out.push(`a=rtpmap:${av1.pt} AV1/90000`, `a=rtcp-fb:${av1.pt} nack`, `a=rtcp-fb:${av1.pt} nack pli`, `a=rtcp-fb:${av1.pt} transport-cc`);
          if (av1.rtx) out.push(`a=rtpmap:${av1.rtx} rtx/90000`, `a=fmtp:${av1.rtx} apt=${av1.pt}`);
        }
        continue;
      }
      out.push(line);
    }
    return out.join(nl);
  };

  const OrigWS = window.WebSocket;
  W.WebSocket = new Proxy(OrigWS, {
    construct(target, args, newTarget) {
      const ws: WebSocket = Reflect.construct(target, args, newTarget);
      sockets.push(ws);
      ws.addEventListener("message", (e) => {
        syMessage("in", e.data);
        dcMessage(ws, e.data);
        record("in", ws, e.data);
      });
      ws.addEventListener("close", () => sockets.splice(sockets.indexOf(ws), 1));
      return ws;
    },
  });
  OrigWS.prototype.send = function (this: WebSocket, data: any) {
    // Discord web annuncia al server solo H.264, VP8 e VP9 (op 1, "codecs"); i client desktop anche
    // AV1 ("AV1X"), e chi trasmette con AV1 (es. encoder hardware recenti) al web non arriva decodificabile
    // ("Impossibile avviare la webcam, errore 2012"). Il browser integrato decodifica AV1: lo
    // aggiungiamo, con priorità dopo H.264, con i payload type della nostra offerta (l'SDP nel
    // messaggio è ridotta e non li contiene). Vedi anche dcAddAv1 per l'SDP di ricezione.
    if (IS_DISCORD && typeof data === "string" && data.includes('"op":1') && /discord\.media/.test(this.url)) {
      try {
        const m = JSON.parse(data);
        const d = m?.d;
        // Discord chiama il suo AV1 "AV1X" (così lo mostra anche il pannello Debug del client desktop)
        if (m?.op === 1 && Array.isArray(d?.codecs) && !d.codecs.some((c: any) => /^AV1/.test(c?.name))) {
          const av1 = dcAv1Pt();
          if (av1) {
            d.codecs.push({ name: "AV1X", type: "video", priority: 1500, payload_type: av1.pt, rtx_payload_type: av1.rtx });
            data = JSON.stringify(m);
            log("discord: annunciato AV1", av1.pt, av1.rtx);
          }
        }
      } catch {}
    }
    if (IS_DISCORD && typeof data === "string" && data.includes('"op":15') && /discord\.media/.test(this.url)) {
      try {
        const m = JSON.parse(data);
        if (m?.op === 15 && m.d && typeof m.d === "object") {
          dcWants.set(this, m.d);
          if (fullRes.on) data = JSON.stringify({ ...m, d: dcBoost(m.d) });
        }
      } catch {}
    }
    syMessage("out", data);
    // identificazione (op 0) sulla connessione vocale: il nostro id utente, e se è una connessione
    // Go Live. I messaggi in arrivo non dicono il tipo: lo dice solo la nostra richiesta
    // (streams: [{ type: "screen" }]), quindi la marchiamo qui, prima che arrivino gli op 12.
    if (IS_DISCORD && typeof data === "string" && /discord\.media/.test(this.url) && data.includes('"op":0')) {
      try {
        const d = JSON.parse(data)?.d;
        if (d?.user_id && !dcSelf) dcSelf = String(d.user_id);
        if (Array.isArray(d?.streams) && d.streams.some((x: any) => x?.type === "screen")) dcStreamWs.add(this);
      } catch {}
    }
    record("out", this, data);
    return origSend.call(this, data);
  };
  // richieste HTTP verso l'API di StreamYard (es. cambio layout), solo col tap attivo
  const origFetch = window.fetch;
  window.fetch = function (this: unknown, input: RequestInfo | URL, init?: RequestInit) {
    if (tap.on) {
      const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
      const method = init?.method ?? (input instanceof Request ? input.method : "GET");
      if (method !== "GET" && /streamyard.com/.test(new URL(url, location.href).host)) {
        const body = typeof init?.body === "string" ? init.body : "";
        tap.log.push({ dir: "http", t: Date.now(), url: method + " " + url, data: body.slice(0, 8000) });
      }
    }
    return origFetch.apply(this, arguments as any);
  } as typeof fetch;
  const origOpen = XMLHttpRequest.prototype.open;
  const origXSend = XMLHttpRequest.prototype.send;
  XMLHttpRequest.prototype.open = function (this: any, method: string, url: string | URL) {
    this.__oy = method + " " + url;
    return origOpen.apply(this, arguments as any);
  } as any;
  XMLHttpRequest.prototype.send = function (this: any, body?: any) {
    if (tap.on && this.__oy && !this.__oy.startsWith("GET"))
      tap.log.push({ dir: "http", t: Date.now(), url: this.__oy, data: typeof body === "string" ? body.slice(0, 8000) : "" });
    return origXSend.apply(this, arguments as any);
  };

  Object.defineProperty(W, "__obsyardTap", {
    value: {
      tap,
      sockets,
      debug: () => ({
        syStreams: [...syStreams], syNames: [...syNames], localStream: [...localStream], offers: offerStream.size,
        dcSsrc: [...dcSsrc], dcActive: [...dcActive], dcNames: [...dcNames], dcSelf, dcSent: dcSent.size, pcs: allPcs.size,
        natives: [...natives].map(([slot, n]) => ({
          slot, on: n.on, audioOn: n.audioOn, video: !!n.video, audio: !!n.audio, vTrack: n.vTrack?.label, aTrack: n.aTrack?.label, aNodes: !!n.aNodes,
          written: n.audio ? Number(new DataView(n.audio.buf).getBigUint64(8, true)) : null,
          seq: n.video ? new DataView(n.video.buf).getUint32(4, true) : null,
          want: resolveSlot(slot).audio?.label ?? null,
        })),
        tracks: [...tracks].map(([id, e]) => ({ id, kind: e.track.kind, origin: e.origin, ssrc: e.ssrc, dcUser: e.dcUser, dcKind: e.dcKind, onAir: onAir(e), label: labelOf(e) })),
      }),
    },
    enumerable: false,
  });

  // -------------------------------------------- nomi ospiti (euristica DOM)
  const guessLabel = (el: Element): string | null => {
    let node: Element | null = el.parentElement;
    for (let depth = 0; node && depth < 6; depth++, node = node.parentElement) {
      const aria = node.getAttribute("aria-label") || node.getAttribute("title");
      if (aria && aria.length <= 40) return aria.trim();
      const text = (node as HTMLElement).innerText;
      if (!text) continue;
      const line = text.split("\n").map((s) => s.trim()).find((s) => s.length >= 2 && s.length <= 40);
      if (line) return line;
    }
    return null;
  };

  const scanLabels = () => {
    if (IS_DISCORD) return; // su Discord i nomi arrivano dagli avatar (dcResolve)
    let changed = false;
    document.querySelectorAll("video,audio").forEach((el) => {
      const so = (el as HTMLMediaElement).srcObject;
      if (!(so instanceof MediaStream)) return;
      const label = guessLabel(el);
      if (!label) return;
      for (const t of so.getTracks()) {
        const id = uidOf(t);
        const e = id ? tracks.get(id) : undefined;
        // con StreamYard il nome arriva dal protocollo: il testo della pagina serve solo come ripiego
        if (e && e.origin === "remote" && !syInfo(e) && e.label !== label) {
          e.label = label;
          changed = true;
        }
      }
    });
    if (changed) {
      publishTracks();
      refreshAll(); // un ospite riconnesso può ora essere ri-agganciato per nome
    }
  };

  // ------------------------------------------------------------ miniature
  const thumbCanvas = document.createElement("canvas");
  thumbCanvas.width = 192;
  thumbCanvas.height = 108;
  const tctx = thumbCanvas.getContext("2d", { alpha: false })!;
  const captures = new WeakMap<MediaStreamTrack, any>();

  const grab = async (e: Entry): Promise<string | null> => {
    const IC = W.ImageCapture;
    if (!IC || e.track.muted) return null;
    let ic = captures.get(e.track);
    if (!ic) captures.set(e.track, (ic = new IC(e.track)));
    const bmp: ImageBitmap | null = await Promise.race([
      ic.grabFrame() as Promise<ImageBitmap>,
      new Promise<null>((r) => setTimeout(() => r(null), 500)),
    ]).catch(() => null);
    if (!bmp) return null;
    if (e.w !== bmp.width || e.h !== bmp.height) {
      e.w = bmp.width;
      e.h = bmp.height;
      publishTracks();
    }
    tctx.drawImage(bmp, 0, 0, thumbCanvas.width, thumbCanvas.height);
    bmp.close();
    return thumbCanvas.toDataURL("image/jpeg", 0.65);
  };

  /** intervallo tra due giri di miniature, dagli fps scelti in Regia (0,5–15) */
  const thumbEvery = () => 1000 / Math.min(15, Math.max(0.2, Number(settings.thumbFps) || 0.5));
  const thumbLoop = async () => {
    const started = performance.now();
    if (thumbsOn) {
      const out: Record<string, string> = {};
      for (const [id, e] of tracks) {
        if (e.track.kind !== "video" || onAir(e) === false) continue;
        const url = await grab(e);
        if (url) out[id] = url;
      }
      if (Object.keys(out).length) invoke("pub_thumbs", { thumbs: out }).catch(() => {});
    }
    // il tempo speso a catturare conta: a fps alti si riparte subito, mai sotto 20 ms
    setTimeout(thumbLoop, Math.max(20, thumbEvery() - (performance.now() - started)));
  };

  // ------------------------------------------------- risoluzione degli slot
  const resolveId = (id: string | null | undefined, kind: string): MediaStreamTrack | null => {
    if (!id) return null;
    const live = tracks.get(id);
    // fuori onda (backstage) = slot vuoto; torna da sé quando rientra in onda
    if (live) return onAir(live) === false ? null : live.track;
    // ospite riconnesso: nuova traccia, stesso nome -> ri-aggancia
    const old = ended.get(id);
    if (old && !/^(Ospite|Schermo) \d+$/.test(old.label)) {
      for (const e of tracks.values())
        if (e.track.kind === kind && e.origin === old.origin && labelOf(e) === old.label && onAir(e) !== false) return e.track;
    }
    return null;
  };

  /** prefisso degli id delle persone senza video (solo audio): "a:<id della traccia audio>" */
  const AUDIO_ONLY = "a:";

  const resolveSlot = (slot: number): { video: MediaStreamTrack | null; audio: MediaStreamTrack | null } => {
    const a = slots[String(slot)] ?? { video: null, audio: "auto" };
    // persona senza video: l'uscita non ha immagine ma porta la sua voce
    if (a.video?.startsWith(AUDIO_ONLY)) {
      const ae = tracks.get(a.video.slice(AUDIO_ONLY.length));
      const own = ae && onAir(ae) !== false ? ae.track : null;
      return { video: null, audio: a.audio === "auto" ? own : resolveId(a.audio, "audio") };
    }
    const video = resolveId(a.video, "video");
    let audio: MediaStreamTrack | null = null;
    if (a.audio === "auto") {
      // audio dello stesso stream StreamYard (o dello stesso MediaStream) della traccia video
      const vid = uidOf(video);
      const ve = vid ? tracks.get(vid) : undefined;
      if (ve) audio = pairedAudio(ve);
    } else audio = resolveId(a.audio, "audio");
    return { video, audio };
  };

  /** l'audio della stessa persona (e dello stesso tipo: camera o schermo) di una traccia video */
  const pairedAudio = (ve: Entry): MediaStreamTrack | null => {
    const same = (e: Entry) =>
      // Discord: stesso utente e stesso tipo (camera o Go Live). Solo questo: Discord può
      // mettere le tracce di tutti nello stesso MediaStream.
      IS_DISCORD && ve.dcUser
        ? e.dcUser === ve.dcUser && e.dcKind === ve.dcKind
        : // Discord, la nostra camera: il microfono che Discord invia (un'altra traccia, elaborata)
          IS_DISCORD && ve.origin === "camera"
          ? e.origin === "camera" && dcSent.has(e.track)
          : (ve.syStream && e.syStream === ve.syStream) ||
            (!!localStream.get(ve.track.id) && localStream.get(e.track.id) === localStream.get(ve.track.id)) ||
            e.streamIds.some((s) => ve.streamIds.includes(s));
    for (const e of tracks.values()) if (e.track.kind === "audio" && same(e)) return e.track;
    return null;
  };

  // ---------------------------------------------------- sender verso OBS
  interface Sub {
    viewerId: number;
    slot: number;
    pc: RTCPeerConnection;
    v: RTCRtpTransceiver;
    a: RTCRtpTransceiver;
    active?: boolean;
  }
  const subs = new Map<number, Sub>();
  const sendTo = (viewerId: number, msg: unknown) => invoke("pub_signal", { viewerId, msg }).catch(() => {});

  const setCodecPrefs = (t: RTCRtpTransceiver) => {
    const caps = RTCRtpSender.getCapabilities?.("video");
    if (!caps || !t.setCodecPreferences) return;
    const mime = "video/" + settings.codec.toLowerCase();
    const score = (c: RTCRtpCodec) => {
      if (c.mimeType.toLowerCase() !== mime) return 0;
      const f = c.sdpFmtpLine ?? "";
      let s = 10;
      if (settings.codec === "H264") {
        if (f.includes("packetization-mode=1")) s += 4;
        if (/profile-level-id=64/.test(f)) s += 2; // High: qualità migliore a parità di bitrate
        else if (/profile-level-id=42e0/.test(f)) s += 1;
      }
      return s;
    };
    const sorted = [...caps.codecs].sort((x, y) => score(y) - score(x));
    try {
      t.setCodecPreferences(sorted);
    } catch (e) {
      log("setCodecPreferences", e);
    }
  };

  /** Bitrate fisso fin dal primo frame: niente ramp-up del BWE su loopback. */
  const mungeAnswer = (sdp: string): string => {
    const kbps = settings.maxBitrateKbps;
    const lines = sdp.split("\r\n");
    const out: string[] = [];
    let section = "";
    const videoPts = new Set<string>();
    const opusPts = new Set<string>();
    for (const l of lines) {
      const m = /^a=rtpmap:(\d+) ([^/]+)/.exec(l);
      if (m) {
        const codec = m[2].toLowerCase();
        if (codec === "opus") opusPts.add(m[1]);
        else if (["h264", "vp8", "vp9", "av1"].includes(codec)) videoPts.add(m[1]);
      }
    }
    for (const l of lines) {
      if (l.startsWith("m=")) section = l.slice(2, 7);
      if (l.startsWith("b=")) continue;
      const f = /^a=fmtp:(\d+) (.*)$/.exec(l);
      if (f && section === "video" && videoPts.has(f[1])) {
        out.push(`${l};x-google-start-bitrate=${Math.round(kbps * 0.8)};x-google-min-bitrate=${Math.round(kbps * 0.4)};x-google-max-bitrate=${kbps}`);
        continue;
      }
      if (f && section === "audio" && opusPts.has(f[1])) {
        out.push(`${l};stereo=1;sprop-stereo=1;maxaveragebitrate=256000`);
        continue;
      }
      out.push(l);
      if (l.startsWith("c=") && section === "video") out.push(`b=AS:${kbps}`);
    }
    return out.join("\r\n");
  };

  const applyParams = async (s: Sub) => {
    try {
      const p = s.v.sender.getParameters() as any;
      if (!p.encodings?.length) p.encodings = [{}];
      Object.assign(p.encodings[0], {
        maxBitrate: settings.maxBitrateKbps * 1000,
        maxFramerate: settings.maxFps,
        scaleResolutionDownBy: 1,
        priority: "high",
        networkPriority: "high",
      });
      p.degradationPreference = "maintain-resolution";
      await s.v.sender.setParameters(p);
      const ap = s.a.sender.getParameters() as any;
      if (!ap.encodings?.length) ap.encodings = [{}];
      ap.encodings[0].maxBitrate = 256000;
      await s.a.sender.setParameters(ap);
    } catch (e) {
      log("setParameters", e);
    }
  };

  /** c'è video solo se la traccia produce frame e la piattaforma non la segna come camera spenta */
  const videoActive = (video: MediaStreamTrack | null) => {
    const ve = tracks.get(uidOf(video) ?? "");
    return !!video && video.enabled && !video.muted && !ve?.stalled && !(ve && syInfo(ve)?.hasVideo === false);
  };

  const applyTracks = async (s: Sub) => {
    const { video, audio } = resolveSlot(s.slot);
    if (s.v.sender.track !== video) await s.v.sender.replaceTrack(video).catch((e) => log("replaceTrack", e));
    if (s.a.sender.track !== audio) await s.a.sender.replaceTrack(audio).catch((e) => log("replaceTrack", e));
    const active = videoActive(video);
    if (s.active !== active) {
      s.active = active;
      sendState(s);
    }
  };

  const sendState = (s: Sub) =>
    sendTo(s.viewerId, { type: "state", video: !!s.active, bg: outputs.find((o) => o.id === s.slot)?.bg ?? settings.emptyBg });

  let refreshQueued = false;
  function refreshAll() {
    if (refreshQueued) return;
    refreshQueued = true;
    queueMicrotask(() => {
      refreshQueued = false;
      subs.forEach((s) => void applyTracks(s));
      natives.forEach((_, slot) => nativeRefresh(slot));
    });
  }

  // ---------------------------------------- uscite verso il plugin OBS nativo
  // Per ogni uscita che la sorgente "OBSYard" di OBS legge, OBSYard ci passa due buffer condivisi
  // WebView2 (video e audio). Qui ci scriviamo i fotogrammi COME IL BROWSER LI HA DECODIFICATI
  // (MediaStreamTrackProcessor + VideoFrame.copyTo), senza ricodifica; Rust li copia nella memoria
  // che il plugin legge. Formato in src-tauri/src/native.rs.
  interface NativeOut {
    on: boolean;
    /** la sorgente in OBS vuole anche l'audio (spento nelle sue proprietà: non lo catturiamo) */
    audioOn?: boolean;
    video?: { buf: ArrayBuffer; slotSize: number };
    audio?: { buf: ArrayBuffer };
    vTrack?: MediaStreamTrack;
    vClone?: MediaStreamTrack;
    vReader?: ReadableStreamDefaultReader<any>;
    busy?: boolean;
    growing?: boolean;
    empty?: boolean;
    aTrack?: MediaStreamTrack;
    aNodes?: { src: AudioNode; proc: AudioWorkletNode | ScriptProcessorNode; mute: GainNode; el: HTMLAudioElement };
  }
  const natives = new Map<number, NativeOut>();
  const nat = (slot: number) => {
    let n = natives.get(slot);
    if (!n) natives.set(slot, (n = { on: false }));
    return n;
  };
  const NV_SLOTS = 3;
  const NV_HEADER = 4096;
  /** VideoFrame.format -> enum video_format di OBS */
  const NV_FORMAT: Record<string, number> = { I420: 1, I420A: 1, NV12: 2, RGBA: 6, RGBX: 6, BGRA: 7, BGRX: 8, I444: 10 };
  const NV_PLANES: Record<string, number> = { I420: 3, I420A: 3, NV12: 2, RGBA: 1, RGBX: 1, BGRA: 1, BGRX: 1, I444: 3 };

  webview?.addEventListener?.("sharedbufferreceived", (e: any) => {
    const d = e.additionalData;
    if (!d?.obsyard) return;
    const buf: ArrayBuffer = e.getBuffer();
    const n = nat(Number(d.slot));
    if (d.obsyard === "video") {
      if (n.video) webview.releaseBuffer?.(n.video.buf);
      n.video = { buf, slotSize: new DataView(buf).getUint32(12, true) };
      n.growing = false;
      n.empty = undefined;
    } else {
      if (n.audio) webview.releaseBuffer?.(n.audio.buf);
      n.audio = { buf };
    }
    log("plugin OBS: buffer", d.obsyard, d.slot);
    nativeRefresh(Number(d.slot));
  });

  /** scrive nel prossimo slot la descrizione di un fotogramma (o "nessuna immagine") e lo pubblica */
  const nvPublish = (v: NonNullable<NativeOut["video"]>, next: number, meta: number[]) => {
    const dv = new DataView(v.buf);
    const m = 16 + next * 64;
    meta.forEach((x, i) => dv.setUint32(m + i * 4, x >>> 0, true));
    dv.setUint32(4, (dv.getUint32(4, true) + 1) >>> 0, true);
  };

  const nvEmpty = (n: NativeOut) => {
    const v = n.video;
    if (!v || n.empty) return;
    const next = (new DataView(v.buf).getUint32(4, true) + 1) % NV_SLOTS;
    nvPublish(v, next, new Array(16).fill(0));
    n.empty = true;
  };

  const nvWrite = async (slot: number, n: NativeOut, frame: any) => {
    const v = n.video;
    if (!v || n.busy) return; // OBS è indietro: questo fotogramma si salta
    let fmt: string = frame.format;
    let opts: any = {};
    if (!fmt || !(fmt in NV_FORMAT)) {
      fmt = "RGBA"; // formato non leggibile da OBS: il browser lo converte
      opts = { format: "RGBA" };
    }
    const size: number = frame.allocationSize(opts);
    if (size > v.slotSize) {
      if (!n.growing) {
        n.growing = true;
        invoke("pub_native_grow", { slot, bytes: size }).catch(() => (n.growing = false));
      }
      return;
    }
    n.busy = true;
    try {
      const next = (new DataView(v.buf).getUint32(4, true) + 1) % NV_SLOTS;
      const layout: { offset: number; stride: number }[] = await frame.copyTo(new Uint8Array(v.buf, NV_HEADER + next * v.slotSize, size), opts);
      if (n.video !== v) return; // buffer cambiato nel frattempo
      const cs = frame.colorSpace ?? {};
      const rgb = NV_PLANES[fmt] === 1;
      const colorspace = rgb ? 3 : cs.matrix === "bt470bg" || cs.matrix === "smpte170m" ? 1 : 2;
      const planes = NV_PLANES[fmt];
      const off = [0, 0, 0, 0], str = [0, 0, 0, 0];
      layout.slice(0, planes).forEach((l, i) => ((off[i] = l.offset), (str[i] = l.stride)));
      const r = frame.visibleRect ?? { width: frame.displayWidth, height: frame.displayHeight };
      nvPublish(v, next, [r.width, r.height, NV_FORMAT[fmt], colorspace, cs.fullRange ? 1 : 0, planes, ...off, ...str, size, 0]);
      n.empty = false;
    } catch (e) {
      log("plugin OBS: copia", e);
    } finally {
      n.busy = false;
    }
  };

  const nvStopVideo = (n: NativeOut) => {
    n.vReader?.cancel().catch(() => {});
    n.vClone?.stop();
    n.vReader = n.vClone = n.vTrack = undefined;
  };

  const nvStartVideo = (slot: number, n: NativeOut, track: MediaStreamTrack) => {
    const Proc = W.MediaStreamTrackProcessor;
    if (!Proc) return log("plugin OBS: MediaStreamTrackProcessor non disponibile");
    const clone = track.clone();
    const reader = new Proc({ track: clone }).readable.getReader();
    Object.assign(n, { vTrack: track, vClone: clone, vReader: reader });
    void (async () => {
      for (;;) {
        const r = await reader.read().catch(() => ({ done: true, value: undefined }));
        if (r.done || !r.value) break;
        const frame = r.value;
        if (n.vReader !== reader) {
          frame.close();
          break;
        }
        // copia in corso: il fotogramma si chiude subito (niente coda di frame aperti)
        if (n.busy) frame.close();
        else void nvWrite(slot, n, frame).finally(() => frame.close());
      }
    })();
  };

  let nvCtx: AudioContext | null = null;
  /**
   * Cattura audio su un AudioWorklet: gira sul thread audio, quindi non perde blocchi quando la
   * pagina (Discord) tiene occupato il thread principale (con ScriptProcessorNode si sentivano
   * scoppiettii: ~0,6% dell'audio mancava). Blocchi da 20 ms, stereo interleaved, inviati alla
   * pagina che li scrive nell'anello: i messaggi si accodano, non si perdono.
   */
  const NV_TAP = `registerProcessor("obsyard-tap", class extends AudioWorkletProcessor {
    constructor() { super(); this.b = new Float32Array(1920); this.n = 0; }
    process(inputs) {
      const i = inputs[0];
      if (!i || !i.length) return true;
      const L = i[0], R = i[1] || i[0];
      for (let k = 0; k < L.length; k++) {
        this.b[this.n * 2] = L[k];
        this.b[this.n * 2 + 1] = R[k];
        if (++this.n === 960) {
          this.port.postMessage(this.b, [this.b.buffer]);
          this.b = new Float32Array(1920);
          this.n = 0;
        }
      }
      return true;
    }
  });`;
  /** il modulo del worklet, caricato una volta per contesto (null: la pagina non lo permette) */
  let nvTapReady: Promise<boolean> | null = null;
  const nvTap = (ctx: AudioContext) =>
    (nvTapReady ??= ctx.audioWorklet
      .addModule(URL.createObjectURL(new Blob([NV_TAP], { type: "text/javascript" })))
      .then(() => true)
      .catch((e) => {
        log("plugin OBS: AudioWorklet non disponibile, uso ScriptProcessor", e);
        return false;
      }));

  /** scrive campioni stereo interleaved nell'anello dell'uscita */
  const nvRing = (n: NativeOut, data: Float32Array, frames: number) => {
    const a = n.audio;
    if (!a) return;
    const dv = new DataView(a.buf);
    const cap = dv.getUint32(4, true);
    if (!cap) return;
    const ring = new Float32Array(a.buf, 64, cap * 2);
    let w = Number(dv.getBigUint64(8, true));
    for (let i = 0; i < frames; i++, w++) {
      const p = (w % cap) * 2;
      ring[p] = data[i * 2];
      ring[p + 1] = data[i * 2 + 1];
    }
    dv.setBigUint64(8, BigInt(w), true);
  };

  const nvStopAudio = (n: NativeOut) => {
    const a = n.aNodes;
    if (a) {
      if (a.proc instanceof AudioWorkletNode) a.proc.port.onmessage = null;
      else a.proc.onaudioprocess = null;
      a.src.disconnect();
      a.proc.disconnect();
      a.mute.disconnect();
      a.el.srcObject = null;
    }
    n.aNodes = n.aTrack = undefined;
  };
  const nvStartAudio = async (n: NativeOut, track: MediaStreamTrack) => {
    n.aTrack = track; // subito: un secondo nativeRefresh non la riavvia mentre il modulo si carica
    nvCtx ??= new AudioContext({ sampleRate: 48000 });
    const ctx = nvCtx;
    void ctx.resume().catch(() => {});
    const worklet = await nvTap(ctx);
    if (n.aTrack !== track) return; // cambiata nel frattempo
    // Chromium: l'audio WebRTC remoto arriva a Web Audio solo se un elemento lo riproduce (muto)
    const el = new Audio();
    el.muted = true;
    el.srcObject = new MediaStream([track]);
    void el.play().catch(() => {});
    const src = ctx.createMediaStreamSource(new MediaStream([track]));
    const mute = ctx.createGain();
    mute.gain.value = 0; // il nodo gira solo se collegato all'uscita: in silenzio
    let proc: AudioWorkletNode | ScriptProcessorNode;
    if (worklet) {
      const node = new AudioWorkletNode(ctx, "obsyard-tap", { numberOfInputs: 1, numberOfOutputs: 1, channelCount: 2, channelCountMode: "explicit" });
      node.port.onmessage = (e) => nvRing(n, e.data as Float32Array, (e.data as Float32Array).length / 2);
      proc = node;
    } else {
      const sp = ctx.createScriptProcessor(1024, 2, 2);
      const buf = new Float32Array(2048);
      sp.onaudioprocess = (ev) => {
        const L = ev.inputBuffer.getChannelData(0);
        const R = ev.inputBuffer.numberOfChannels > 1 ? ev.inputBuffer.getChannelData(1) : L;
        for (let i = 0; i < L.length; i++) ((buf[i * 2] = L[i]), (buf[i * 2 + 1] = R[i]));
        nvRing(n, buf, L.length);
      };
      proc = sp;
    }
    src.connect(proc);
    proc.connect(mute);
    mute.connect(ctx.destination);
    n.aNodes = { src, proc, mute, el };
  };

  /** allinea la cattura di un'uscita a cosa mostra ora (cambio sorgente: subito, senza attese) */
  function nativeRefresh(slot: number) {
    const n = natives.get(slot);
    if (!n) return;
    const { video, audio } = n.on ? resolveSlot(slot) : { video: null, audio: null };
    const wantV = n.video && video && videoActive(video) ? video : null;
    if (wantV !== (n.vTrack ?? null)) {
      nvStopVideo(n);
      if (wantV) nvStartVideo(slot, n, wantV);
    }
    if (!wantV && n.on) nvEmpty(n);
    const wantA = n.audio && n.audioOn !== false && audio ? audio : null;
    if (wantA !== (n.aTrack ?? null)) {
      nvStopAudio(n);
      if (wantA) void nvStartAudio(n, wantA);
    }
  }

  const closeSub = (viewerId: number) => {
    const s = subs.get(viewerId);
    if (s) {
      s.pc.close();
      subs.delete(viewerId);
    }
  };

  const openSub = async (viewerId: number, slot: number) => {
    closeSub(viewerId);
    const pc = new OrigPC({ bundlePolicy: "max-bundle", iceServers: [] });
    const v = pc.addTransceiver("video", { direction: "sendonly" });
    const a = pc.addTransceiver("audio", { direction: "sendonly" });
    setCodecPrefs(v);
    const s: Sub = { viewerId, slot, pc, v, a };
    subs.set(viewerId, s);
    pc.onicecandidate = (e) => e.candidate && sendTo(viewerId, { type: "ice", candidate: e.candidate.toJSON() });
    await applyTracks(s);
    const offer = await pc.createOffer();
    await pc.setLocalDescription(offer);
    sendTo(viewerId, { type: "offer", sdp: pc.localDescription!.sdp });
    sendState(s);
  };

  const onSignal = async (viewerId: number, msg: any) => {
    const s = subs.get(viewerId);
    if (!s) return;
    if (msg.type === "answer") {
      await s.pc.setRemoteDescription({ type: "answer", sdp: mungeAnswer(msg.sdp) });
      await applyParams(s);
    } else if (msg.type === "ice" && msg.candidate) {
      await s.pc.addIceCandidate(msg.candidate).catch(() => {});
    }
  };

  // ------------------------------------- ospiti a piena risoluzione
  // Ogni ospite codifica a (altezza % del suo riquadro) × risoluzione della diretta.
  // Con un layout personalizzato a riquadri tutti a piena altezza, inviano al massimo.
  // Cambiamo layout cliccando il pulsante di StreamYard (niente messaggi iniettati).
  const fullRes = { on: false, layout: "OBSYard", prev: null as string | null, status: "off" };
  const btnLabel = (b: Element) => (b.getAttribute("aria-label") || (b as HTMLElement).innerText || "").trim();
  const findLayout = (name: string) => {
    const n = name.trim().toLowerCase();
    return [...document.querySelectorAll<HTMLButtonElement>("button[aria-pressed]")].find((b) =>
      [b.getAttribute("aria-label"), b.innerText, b.title].some((x) => x?.trim().toLowerCase() === n),
    );
  };
  /** il pulsante premuto nello stesso gruppo dei pulsanti layout */
  const pressedSibling = (b: HTMLElement) => {
    for (let p = b.parentElement, i = 0; p && i < 8; p = p.parentElement, i++) {
      const all = [...p.querySelectorAll<HTMLButtonElement>("button[aria-pressed]")];
      if (all.length >= 3) return all.find((x) => x.getAttribute("aria-pressed") === "true") ?? null;
    }
    return null;
  };
  /** esito dell'ultima scrittura del layout: "" già a posto, "created"/"updated" (serve ricaricare), altrimenti l'errore */
  let layoutWrite = "";
  const needsReload = () => layoutWrite === "created" || layoutWrite === "updated";
  const reportFullRes = () => {
    if (IS_DISCORD) return; // su Discord lo stato lo diamo subito, in recv
    if (fullRes.status === "busy") return;
    const target = findLayout(fullRes.layout);
    const status = !fullRes.on
      ? "off"
      : layoutWrite && !needsReload()
        ? "error:" + layoutWrite
        : needsReload()
          ? layoutWrite // lo studio usa la copia letta al caricamento: va ricaricato
          : !target
            ? "missing"
            : target.getAttribute("aria-pressed") === "true"
              ? "on"
              : "manual"; // acceso, ma hai scelto a mano un altro layout: lo rispettiamo
    if (status !== fullRes.status) {
      fullRes.status = status;
      invoke("pub_fullres", { status }).catch(() => {});
    }
  };

  const csrf = () => decodeURIComponent(/(?:^|; )csrfToken=([^;]*)/.exec(document.cookie)?.[1] ?? "");
  /** id fissi (24 caratteri come quelli di StreamYard): lo stesso layout produce sempre lo stesso contenuto */
  const cid = (i: number) => "obsyardcam" + String(i + 1).padStart(14, "0");
  /** una colonna a piena altezza per ogni camera che il piano può avere in onda */
  const desiredLayout = () => {
    const n = Math.max(1, Math.min(maxPublicCams, 20));
    const w = 100 / n;
    const ids = Array.from({ length: n }, (_, i) => cid(i));
    return {
      containers: Object.fromEntries(
        ids.map((id, i) => [id, { id, x: w * i, y: 0, width: w, height: 100, fit: "fill", kind: "camera" }]),
      ) as Record<string, { id: string; x: number; y: number; width: number; height: number; fit: string; kind: string }>,
      containersPriorityOrder: ids,
      containersSortOrder: ids,
    };
  };
  const sameLayout = (a: any, b: ReturnType<typeof desiredLayout>) => {
    const ka = Object.keys(a?.containers ?? {}).sort();
    const kb = Object.keys(b.containers).sort();
    if (ka.join() !== kb.join()) return false;
    return kb.every((k) => {
      const x = a.containers[k], y = b.containers[k];
      return ["x", "y", "width", "height"].every((f) => Math.abs(x[f] - (y as any)[f]) < 0.01) && x.kind === y.kind && x.fit === y.fit;
    }) && JSON.stringify(a.containersPriorityOrder) === JSON.stringify(b.containersPriorityOrder);
  };
  /**
   * Scrive il layout personalizzato SOLO se manca o è diverso da come deve essere (prima volta,
   * o modificato a mano). Lo studio tiene in memoria i layout letti al caricamento: se lo
   * riscrivessimo ogni volta, la sua copia non corrisponderebbe più e StreamYard ignorerebbe il cambio.
   * Stessa API dell'editor di StreamYard: GET/POST /api/workspaces/{ws}/layouts[/{id}].
   * Ritorna "" (già a posto), "created", "updated" (serve ricaricare lo studio) o un errore.
   */
  const writeLayout = async (): Promise<string> => {
    if (!workspaceId) return "workspace StreamYard non ancora noto: entra nello studio";
    const base = `/api/workspaces/${workspaceId}/layouts`;
    const list: any[] = await origFetch(base, { credentials: "include" }).then((r) => (r.ok ? r.json() : []));
    const existing = list.find((l) => l.name?.trim().toLowerCase() === fullRes.layout.trim().toLowerCase());
    const want = desiredLayout();
    if (existing && sameLayout(existing, want)) return "";
    const r = await origFetch(existing ? `${base}/${existing.id}` : base, {
      method: "POST",
      credentials: "include",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name: existing?.name ?? fullRes.layout, ...want, csrfToken: csrf() }),
    });
    if (!r.ok) return `StreamYard ha rifiutato il layout (${r.status})`;
    return existing ? "updated" : "created";
  };

  const applyFullRes = async () => {
    fullRes.status = "busy";
    invoke("pub_fullres", { status: "" }).catch(() => {});
    if (fullRes.on) {
      // ogni attivazione riscrive il layout (anche se nel frattempo è stato modificato)
      layoutWrite = await writeLayout().catch((e) => String(e));
      let target = findLayout(fullRes.layout);
      for (let i = 0; !target && i < 10; i++) {
        await new Promise((r) => setTimeout(r, 300));
        target = findLayout(fullRes.layout);
      }
      // appena riscritto: la copia in memoria dello studio è vecchia e StreamYard ignorerebbe il cambio
      if (!needsReload() && target && target.getAttribute("aria-pressed") !== "true") {
        const cur = pressedSibling(target);
        if (cur) fullRes.prev = btnLabel(cur);
        target.click();
      }
    } else {
      layoutWrite = "";
      const target = findLayout(fullRes.layout);
      if (fullRes.prev) {
        const back = findLayout(fullRes.prev);
        if (back && target?.getAttribute("aria-pressed") === "true") back.click();
        fullRes.prev = null;
      }
    }
    fullRes.status = "";
    setTimeout(reportFullRes, 600);
  };
  setInterval(reportFullRes, 2000);

  // ----------------------------------------------------- messaggi da Rust
  const recv = (m: any) => {
    switch (m.type) {
      case "init":
        settings = m.settings;
        slots = m.slots;
        outputs = m.outputs ?? [];
        thumbsOn = m.thumbs;
        subs.forEach((s) => closeSub(s.viewerId));
        publishTracks();
        break;
      case "viewer-join":
        void openSub(m.viewerId, m.slot);
        break;
      case "viewer-leave":
        closeSub(m.viewerId);
        break;
      case "signal":
        void onSignal(m.viewerId, m.msg);
        break;
      case "slots":
        slots = m.slots;
        refreshAll();
        break;
      case "settings": {
        const codecChanged = m.settings.codec !== settings.codec;
        settings = m.settings;
        if (codecChanged) {
          // il codec richiede una nuova negoziazione
          for (const s of [...subs.values()]) {
            sendTo(s.viewerId, { type: "reset" });
            void openSub(s.viewerId, s.slot);
          }
        } else
          subs.forEach((s) => {
            void applyParams(s);
            sendState(s); // sfondo senza video globale
          });
        break;
      }
      case "outputs": {
        outputs = m.outputs;
        subs.forEach(sendState); // sfondo nero/trasparente per uscita
        break;
      }
      case "thumbs":
        thumbsOn = m.on;
        break;
      case "native": {
        // il plugin OBS ha cominciato (o smesso) a leggere un'uscita
        const n = nat(Number(m.slot));
        n.on = !!m.on;
        n.audioOn = m.audio !== false;
        if (!n.on) {
          nvStopVideo(n);
          nvStopAudio(n);
        }
        nativeRefresh(Number(m.slot));
        break;
      }
      case "fullRes":
        if (IS_DISCORD) {
          if (m.on !== fullRes.on) {
            fullRes.on = m.on;
            dcResendWants();
          }
          fullRes.status = m.on ? "on" : "off";
          invoke("pub_fullres", { status: fullRes.status }).catch(() => {});
          break;
        }
        if (m.on !== fullRes.on || m.layout !== fullRes.layout) {
          fullRes.on = m.on;
          fullRes.layout = m.layout;
          void applyFullRes();
        }
        break;
    }
  };

  Object.defineProperty(W, "__obsyard", { value: { recv }, enumerable: false });

  // ------------------------------------------------------------------ avvio
  const dbg = (m: string) => {
    if (location.hostname === "127.0.0.1") fetch("/api/log", { method: "POST", body: m }).catch(() => {});
  };
  let tries = 0;
  const ready = () => {
    if (!W.__TAURI_INTERNALS__) {
      if (++tries % 50 === 0) dbg("__TAURI_INTERNALS__ assente dopo " + tries + " tentativi");
      return void setTimeout(ready, 100);
    }
    invoke("pub_ready").then(() => dbg("pub_ready ok"), (e) => {
      console.warn("[obsyard] pub_ready", e);
      dbg("pub_ready errore: " + e);
    });
  };
  dbg("script caricato");
  ready();
  setInterval(scanLabels, 2000);

  // Frame in arrivo, solo per le tracce remote attualmente su uno slot (costo: 1 getStats ogni 500 ms).
  // Chrome non segna subito "muted" una traccia remota che smette di ricevere frame.
  const STALL_MS = 1200;
  setInterval(async () => {
    const used = new Set<MediaStreamTrack>();
    subs.forEach((s) => s.v.sender.track && used.add(s.v.sender.track));
    // anche quelle assegnate ma nascoste perché ferme: devono poter tornare visibili
    for (const n of Object.keys(slots)) {
      const t = resolveSlot(Number(n)).video;
      if (t) used.add(t);
    }
    let changed = false;
    const now = performance.now();
    for (const t of used) {
      const e = tracks.get(uidOf(t) ?? "");
      if (!e?.receiver) continue;
      let frames = 0;
      try {
        (await e.receiver.getStats()).forEach((s: any) => {
          if (s.type === "inbound-rtp" && s.kind === "video") frames = s.framesDecoded ?? 0;
        });
      } catch {
        continue;
      }
      if (frames !== e.frames) {
        e.frames = frames;
        e.lastFrameAt = now;
      }
      const stalled = now - (e.lastFrameAt ?? now) > STALL_MS;
      if (stalled !== !!e.stalled) {
        e.stalled = stalled;
        changed = true;
      }
    }
    if (changed) refreshAll();
  }, 500);
  setTimeout(thumbLoop, 1500);
  // pulizia di sicurezza: tracce terminate senza evento "ended"
  setInterval(() => {
    for (const [id, e] of tracks) if (e.track.readyState === "ended") e.track.dispatchEvent(new Event("ended")), tracks.delete(id);
  }, 3000);
})();
