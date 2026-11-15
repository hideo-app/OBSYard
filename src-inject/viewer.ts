/**
 * Pagina caricata da OBS come "Fonte browser": http://127.0.0.1:7878/slot/N
 * Riceve via WebRTC (loopback) la sorgente assegnata allo slot, con latenza minima.
 *
 * Query string:
 *   audio=0      solo video
 *   fit=cover    riempi il riquadro (default: contain)
 *   bg=000000    sfondo pieno invece che trasparente
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
          video.classList.toggle("on", !!m.video);
          break;
        case "reset":
          closePc();
          video.classList.remove("on");
          break;
      }
    };
    ws.onclose = () => {
      closePc();
      video.classList.remove("on");
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
