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
import type { Settings, SlotAssign, TrackInfo, TrackOrigin } from "./types";

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
    w?: number;
    h?: number;
  }
  const tracks = new Map<string, Entry>();
  /** tracce terminate: servono per ri-agganciare automaticamente lo stesso ospite se riconnette */
  const ended = new Map<string, { kind: string; origin: TrackOrigin; label: string }>();
  let slots: Record<string, SlotAssign> = {};
  let settings: Settings = { codec: "H264", maxBitrateKbps: 12000, maxFps: 60 };
  let thumbsOn = true;
  const counters: Record<TrackOrigin, number> = { remote: 0, camera: 0, screen: 0 };

  // -------------------------------------------------------- pubblicazione
  let publishTimer = 0;
  const publishTracks = () => {
    if (publishTimer) return;
    publishTimer = window.setTimeout(() => {
      publishTimer = 0;
      const list: TrackInfo[] = [...tracks].map(([id, e]) => ({
        id,
        kind: e.track.kind as "video" | "audio",
        origin: e.origin,
        label: e.label,
        streamIds: e.streamIds,
        muted: e.track.muted,
        w: e.w,
        h: e.h,
      }));
      invoke("pub_tracks", { tracks: list }).catch(() => {});
    }, 80);
  };

  const register = (track: MediaStreamTrack, origin: TrackOrigin, streams: readonly MediaStream[] = []) => {
    if (tracks.has(track.id) || track.readyState === "ended") return;
    const n = ++counters[origin];
    const base = origin === "remote" ? "Ospite" : origin === "camera" ? (track.kind === "video" ? "Camera locale" : "Microfono locale") : "Schermo";
    tracks.set(track.id, { track, origin, label: `${base} ${n}`, streamIds: streams.map((s) => s.id) });
    const onEnd = () => {
      const e = tracks.get(track.id);
      if (!e) return;
      tracks.delete(track.id);
      ended.set(track.id, { kind: track.kind, origin: e.origin, label: e.label });
      refreshAll();
      publishTracks();
    };
    track.addEventListener("ended", onEnd);
    track.addEventListener("mute", publishTracks);
    track.addEventListener("unmute", publishTracks);
    log("traccia", origin, track.kind, track.id);
    refreshAll();
    publishTracks();
  };

  // ------------------------------------------------------------ hook WebRTC
  // Proxy (non sottoclasse) così newTarget, instanceof e sottoclassi della pagina restano intatti.
  const HookedPC = new Proxy(OrigPC, {
    construct(target, args, newTarget) {
      const pc: RTCPeerConnection = Reflect.construct(target, args, newTarget);
      pc.addEventListener("track", (e: RTCTrackEvent) => register(e.track, "remote", e.streams));
      return pc;
    },
  });
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
        const e = tracks.get(t.id);
        if (e && e.origin === "remote" && e.label !== label) {
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
        if (e.track.kind !== "video") continue;
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
    if (live) return live.track;
    // ospite riconnesso: nuova traccia, stesso nome -> ri-aggancia
    const old = ended.get(id);
    if (old && !/^(Ospite|Schermo) \d+$/.test(old.label)) {
      for (const e of tracks.values())
        if (e.track.kind === kind && e.origin === old.origin && e.label === old.label) return e.track;
    }
    return null;
  };

  const resolveSlot = (slot: number): { video: MediaStreamTrack | null; audio: MediaStreamTrack | null } => {
    const a = slots[String(slot)] ?? { video: null, audio: "auto" };
    const video = resolveId(a.video, "video");
    let audio: MediaStreamTrack | null = null;
    if (a.audio === "auto") {
      // audio dello stesso MediaStream della traccia video (stesso ospite)
      const ve = video && tracks.get(video.id);
      if (ve) {
        for (const e of tracks.values())
          if (e.track.kind === "audio" && e.streamIds.some((s) => ve.streamIds.includes(s))) {
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
    const active = !!video;
    if (s.active !== active) {
      s.active = active;
      sendTo(s.viewerId, { type: "state", video: active });
    }
  };

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
    sendTo(viewerId, { type: "state", video: !!s.active });
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

  // ----------------------------------------------------- messaggi da Rust
  const recv = (m: any) => {
    switch (m.type) {
      case "init":
        settings = m.settings;
        slots = m.slots;
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
        } else subs.forEach((s) => void applyParams(s));
        break;
      }
      case "thumbs":
        thumbsOn = m.on;
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
  setTimeout(thumbLoop, 1500);
  // pulizia di sicurezza: tracce terminate senza evento "ended"
  setInterval(() => {
    for (const [id, e] of tracks) if (e.track.readyState === "ended") e.track.dispatchEvent(new Event("ended")), tracks.delete(id);
  }, 3000);
})();
