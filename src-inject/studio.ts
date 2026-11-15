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
  /** undefined = non sappiamo (es. pagina di test): la consideriamo visibile */
  const onAir = (e: Entry): boolean | undefined => {
    const s = syInfo(e);
    return s ? s.privacy === "public" : undefined;
  };
  const labelOf = (e: Entry) => {
    const s = syInfo(e);
    const name = s && syNames.get(s.clientId);
    if (!name) return e.label;
    return s!.kind === "webcam" ? name : `${name} (${s!.kind === "screen" ? "schermo" : s!.kind})`;
  };
  /** tracce terminate: servono per ri-agganciare automaticamente lo stesso ospite se riconnette */
  const ended = new Map<string, { kind: string; origin: TrackOrigin; label: string }>();
  let slots: Record<string, SlotAssign> = {};
  let settings: Settings = { codec: "H264", maxBitrateKbps: 12000, maxFps: 60, fullResLayout: "OBSYard", autoAdd: false, emptyBg: "transparent", priority: [] };
  let outputs: Output[] = [];
  let thumbsOn = true;
  const counters: Record<TrackOrigin, number> = { remote: 0, camera: 0, screen: 0 };

  // -------------------------------------------------------- pubblicazione
  let publishTimer = 0;
  /** schermo condiviso (nostro o di un ospite), non una webcam */
  const isScreen = (e: Entry) => {
    if (e.origin === "screen") return true;
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
      pc.addEventListener("track", (e: RTCTrackEvent) => register(e.track, "remote", e.streams, pcStream.get(pc), e.receiver));
      return pc;
    },
  });
  // lega la PeerConnection allo stream StreamYard: l'offerta Janus arriva col suo streamId
  const origSRD = OrigPC.prototype.setRemoteDescription;
  OrigPC.prototype.setRemoteDescription = function (this: RTCPeerConnection, d?: RTCSessionDescriptionInit) {
    const sid = d?.sdp && offerStream.get(originLine(d.sdp));
    if (sid) pcStream.set(this, sid);
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
    if (!tap.on || typeof data !== "string") return;
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

  const OrigWS = window.WebSocket;
  W.WebSocket = new Proxy(OrigWS, {
    construct(target, args, newTarget) {
      const ws: WebSocket = Reflect.construct(target, args, newTarget);
      sockets.push(ws);
      ws.addEventListener("message", (e) => {
        syMessage("in", e.data);
        record("in", ws, e.data);
      });
      ws.addEventListener("close", () => sockets.splice(sockets.indexOf(ws), 1));
      return ws;
    },
  });
  const origSend = OrigWS.prototype.send;
  OrigWS.prototype.send = function (this: WebSocket, data: any) {
    syMessage("out", data);
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
    value: { tap, sockets, debug: () => ({ syStreams: [...syStreams], syNames: [...syNames], localStream: [...localStream], offers: offerStream.size }) },
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

  const thumbLoop = async () => {
    if (thumbsOn) {
      const out: Record<string, string> = {};
      for (const [id, e] of tracks) {
        if (e.track.kind !== "video" || onAir(e) === false) continue;
        const url = await grab(e);
        if (url) out[id] = url;
      }
      if (Object.keys(out).length) invoke("pub_thumbs", { thumbs: out }).catch(() => {});
    }
    setTimeout(thumbLoop, 2000);
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

  const resolveSlot = (slot: number): { video: MediaStreamTrack | null; audio: MediaStreamTrack | null } => {
    const a = slots[String(slot)] ?? { video: null, audio: "auto" };
    const video = resolveId(a.video, "video");
    let audio: MediaStreamTrack | null = null;
    if (a.audio === "auto") {
      // audio dello stesso stream StreamYard (o dello stesso MediaStream) della traccia video
      const vid = uidOf(video);
      const ve = vid ? tracks.get(vid) : undefined;
      if (ve) {
        const same = (e: Entry) =>
          (ve.syStream && e.syStream === ve.syStream) ||
          (!!localStream.get(ve.track.id) && localStream.get(e.track.id) === localStream.get(ve.track.id)) ||
          e.streamIds.some((s) => ve.streamIds.includes(s));
        for (const e of tracks.values())
          if (e.track.kind === "audio" && same(e)) {
            audio = e.track;
            break;
          }
      }
    } else audio = resolveId(a.audio, "audio");
    return { video, audio };
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

  const applyTracks = async (s: Sub) => {
    const { video, audio } = resolveSlot(s.slot);
    if (s.v.sender.track !== video) await s.v.sender.replaceTrack(video).catch((e) => log("replaceTrack", e));
    if (s.a.sender.track !== audio) await s.a.sender.replaceTrack(audio).catch((e) => log("replaceTrack", e));
    // c'è video solo se la traccia produce frame e StreamYard non la segna come camera spenta
    const ve = tracks.get(uidOf(video) ?? "");
    const active =
      !!video && video.enabled && !video.muted && !ve?.stalled && !(ve && syInfo(ve)?.hasVideo === false);
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
    });
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
      case "fullRes":
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
