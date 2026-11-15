/**
 * Pagina caricata da OBS come "Fonte browser": http://127.0.0.1:7878/slot/N
 * Riceve via WebRTC (loopback) la sorgente assegnata allo slot, con latenza minima.
 *
 * Query string:
 *   audio=0      solo video
 *   fit=cover    riempi il riquadro (default: contain)
 *   bg=000000    sfondo pieno invece che trasparente
 *   lt=0         senza sottopancia su questa fonte
 *   debug=1      overlay con fps / risoluzione / codec / ritardo
 */
(() => {
  const slot: number = (window as any).__SLOT;
  const qs = new URLSearchParams(location.search);
  const wantAudio = qs.get("audio") !== "0";
  const video = document.getElementById("v") as HTMLVideoElement;
  video.style.objectFit = qs.get("fit") ?? "contain";
  if (qs.get("bg")) document.body.style.background = "#" + qs.get("bg");
  video.muted = !wantAudio;

  let ws: WebSocket | null = null;
  let pc: RTCPeerConnection | null = null;

  const send = (m: unknown) => ws?.readyState === WebSocket.OPEN && ws.send(JSON.stringify(m));

  const closePc = () => {
    pc?.close();
    pc = null;
    video.srcObject = null;
  };

  const lowLatency = (r: RTCRtpReceiver) => {
    const rr = r as any;
    try {
      if ("jitterBufferTarget" in rr) rr.jitterBufferTarget = 0; // Chromium >= 114
      if ("playoutDelayHint" in rr) rr.playoutDelayHint = 0; // Chromium più vecchi (CEF di OBS)
    } catch {}
  };

  // ------------------------------------------------------------- tally
  // OBS dice a una Fonte browser se è in programma (active) o visibile (anche solo in anteprima)
  // soltanto quando cambia, mai al caricamento: teniamo l'ultimo stato e lo rimandiamo a ogni
  // connessione, così la Regia non resta con un tally vecchio.
  const tally = { active: false, visible: false };
  const sendTally = () => send({ type: "tally", ...tally });
  window.addEventListener("obsSourceActiveChanged", (e: any) => {
    tally.active = !!e.detail?.active;
    if (tally.active) tally.visible = true;
    sendTally();
  });
  window.addEventListener("obsSourceVisibleChanged", (e: any) => {
    tally.visible = !!e.detail?.visible;
    if (!tally.visible) tally.active = false;
    sendTally();
  });

  // ------------------------------------------------------- sottopancia
  // Stesse classi e variabili CSS di guestroom.live: .lower-third[data-state][data-source-kind],
  // .lower-third-name, .lower-third-title, --lt-*. Si vede solo mentre l'uscita ha video.
  const wantLt = qs.get("lt") !== "0";
  let lt = { name: "", title: "", kind: "camera" };
  let ltEl: HTMLElement | null = null;
  let ltShown = "";
  let ltTimer = 0;
  let hasVideo = false;

  const renderLt = () => {
    const want = wantLt && hasVideo && lt.name ? JSON.stringify(lt) : "";
    if (want === ltShown) return;
    ltShown = want;
    clearTimeout(ltTimer);
    const show = () => {
      if (!want) {
        ltEl?.remove();
        ltEl = null;
        return;
      }
      if (!ltEl) {
        ltEl = document.createElement("div");
        ltEl.className = "lower-third";
        ltEl.dataset.state = "out";
        document.body.appendChild(ltEl);
      }
      const el = ltEl;
      el.dataset.sourceKind = lt.kind;
      el.replaceChildren();
      const add = (cls: string, text: string) => {
        const d = document.createElement("div");
        d.className = cls;
        d.textContent = text;
        el.appendChild(d);
      };
      add("lower-third-name", lt.name);
      if (lt.title) add("lower-third-title", lt.title);
      void el.offsetWidth; // parte da "out", poi entra
      el.dataset.state = "in";
    };
    if (ltEl) {
      // esce, poi cambia testo (o sparisce)
      ltEl.dataset.state = "out";
      ltTimer = window.setTimeout(show, 380);
    } else show();
  };

  const setVideoOn = (on: boolean) => {
    video.classList.toggle("on", on);
    hasVideo = on;
    renderLt();
  };

  // -------------------------------------------- dettagli della connessione
  // Chiesti dalla Regia (messaggio "stats") mentre il pannello dell'uscita è aperto.
  let statBytes = 0;
  let statAt = 0;
  const sendStats = async () => {
    const out: Record<string, unknown> = { state: pc?.connectionState ?? "closed", program: tally.active, visible: tally.visible };
    if (pc) {
      const stats = await pc.getStats();
      stats.forEach((s: any) => {
        if (s.type === "inbound-rtp" && s.kind === "video") {
          const now = performance.now();
          if (statAt && s.bytesReceived >= statBytes) out.kbps = Math.round(((s.bytesReceived - statBytes) * 8) / (now - statAt));
          statBytes = s.bytesReceived;
          statAt = now;
          Object.assign(out, {
            w: s.frameWidth,
            h: s.frameHeight,
            fps: s.framesPerSecond ?? 0,
            codec: String(stats.get(s.codecId)?.mimeType ?? "").replace("video/", ""),
            decoder: s.decoderImplementation,
            jitterMs: s.jitterBufferEmittedCount ? Math.round((s.jitterBufferDelay / s.jitterBufferEmittedCount) * 1000) : undefined,
            lost: s.packetsLost ?? 0,
            dropped: s.framesDropped ?? 0,
            freezes: s.freezeCount ?? 0,
          });
        }
        if (s.type === "candidate-pair" && s.nominated && s.state === "succeeded")
          out.rttMs = Math.round((s.currentRoundTripTime ?? 0) * 10000) / 10;
      });
      out.audio = ((video.srcObject as MediaStream | null)?.getAudioTracks().length ?? 0) > 0;
    }
    send({ type: "stats", stats: out });
  };

  const play = () =>
    video.play().catch(() => {
      // fuori da OBS l'autoplay con audio può essere bloccato
      video.muted = true;
      return video.play().catch(() => {});
    });

  const onOffer = async (sdp: string) => {
    closePc();
    const p = (pc = new RTCPeerConnection({ iceServers: [], bundlePolicy: "max-bundle" }));
    p.onicecandidate = (e) => e.candidate && send({ type: "ice", candidate: e.candidate.toJSON() });
    p.ontrack = () => {
      p.getReceivers().forEach(lowLatency);
      const t = p.getReceivers().map((r) => r.track).filter((t) => wantAudio || t.kind === "video");
      video.srcObject = new MediaStream(t);
      play();
    };
    p.onconnectionstatechange = () => {
      if (p.connectionState === "failed") ws?.close(); // riconnessione completa
    };
    await p.setRemoteDescription({ type: "offer", sdp });
    const answer = await p.createAnswer();
    await p.setLocalDescription(answer);
    send({ type: "answer", sdp: p.localDescription!.sdp });
  };

  const connect = () => {
    ws = new WebSocket(`ws://${location.host}/ws?slot=${slot}`);
    ws.onopen = sendTally;
    ws.onmessage = async (ev) => {
      const m = JSON.parse(ev.data);
      switch (m.type) {
        case "offer":
          await onOffer(m.sdp);
          break;
        case "ice":
          await pc?.addIceCandidate(m.candidate).catch(() => {});
          break;
        case "state":
          // senza video: niente ultimo fotogramma, ma lo sfondo scelto in OBSYard
          // (?bg= nell'URL della fonte ha la precedenza)
          setVideoOn(!!m.video);
          if (!qs.get("bg")) document.body.style.background = m.bg === "black" ? "#000" : "transparent";
          break;
        case "lt":
          lt = { name: m.name ?? "", title: m.title ?? "", kind: m.kind ?? "camera" };
          renderLt();
          break;
        case "stats":
          void sendStats();
          break;
        case "reset":
          closePc();
          setVideoOn(false);
          break;
      }
    };
    ws.onclose = () => {
      closePc();
      setVideoOn(false);
      setTimeout(connect, 1000);
    };
  };
  connect();

  // ------------------------------------------------------------- debug
  if (qs.get("debug") === "1") {
    const box = document.createElement("pre");
    box.style.cssText =
      "position:fixed;left:8px;top:8px;margin:0;padding:6px 8px;font:12px/1.4 monospace;color:#0f0;background:rgba(0,0,0,.7);z-index:9";
    document.body.appendChild(box);
    let lastBytes = 0;
    setInterval(async () => {
      if (!pc) return void (box.textContent = `slot ${slot}: in attesa`);
      const stats = await pc.getStats();
      let txt = `slot ${slot} · ${pc.connectionState}`;
      stats.forEach((s: any) => {
        if (s.type === "inbound-rtp" && s.kind === "video") {
          const codec = stats.get(s.codecId)?.mimeType ?? "?";
          const jb = s.jitterBufferEmittedCount ? ((s.jitterBufferDelay / s.jitterBufferEmittedCount) * 1000).toFixed(0) : "?";
          const mbps = ((s.bytesReceived - lastBytes) * 8) / 1e6;
          lastBytes = s.bytesReceived;
          txt += `\n${s.frameWidth ?? "?"}x${s.frameHeight ?? "?"} @ ${s.framesPerSecond ?? 0} fps · ${mbps.toFixed(1)} Mbps\n${codec} · dec ${s.decoderImplementation ?? "n/d"}\njitter buffer ${jb} ms · persi ${s.packetsLost ?? 0}`;
        }
        if (s.type === "candidate-pair" && s.nominated && s.state === "succeeded")
          txt += `\nRTT ${((s.currentRoundTripTime ?? 0) * 1000).toFixed(1)} ms`;
      });
      const audio = (video.srcObject as MediaStream | null)?.getAudioTracks().length ?? 0;
      box.textContent = txt + `\naudio: ${audio ? "sì" : "no"}`;
    }, 1000);
  }
})();
