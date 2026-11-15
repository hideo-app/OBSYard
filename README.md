# OBSYard

Tauri v2 app (Rust + Vue 3/TS) that opens StreamYard and forwards **every single source** (guests, local camera, screen shares) to OBS as a *Browser Source*.

## How it works

```
┌─ "studio" window (WebView2) ─────────────┐        ┌─ OBS ───────────────────────┐
│ streamyard.com                            │        │ Browser Source              │
│  + studio.js (injected before the page)    │ WebRTC │ http://127.0.0.1:7878/slot/1│
│    hook RTCPeerConnection / getUserMedia  │══════▶ │  viewer.js                  │
│    1 sendonly PeerConnection/OBS source   │loopback│  jitter buffer = 0          │
└──────────────┬────────────────────────────┘        └──────────────┬──────────────┘
               │ IPC (SDP/ICE only, a few KB)                       │ WebSocket
               └──────────────► Rust: hub + server axum ◄───────────┘
                                   ▲  events
                       "control" window (Vue)
```

Performance choices:
- **Frames never go through Rust/IPC**: video goes from the WebView to OBS over WebRTC on loopback (RTT ~1 ms).
- **H.264 with the hardware encoder** (Media Foundation), High profile preferred; VP8/VP9/AV1 selectable.
- **Fixed bitrate from the first frame** (`x-google-start/min/max-bitrate`, `b=AS`): no congestion-control ramp-up.
- `degradationPreference = maintain-resolution`: under heavy load fps drop, not resolution.
- **On the OBS side** `jitterBufferTarget = 0` / `playoutDelayHint = 0`: minimal latency.
- **Switching the source of a slot = `replaceTrack()`**: no renegotiation, no hiccup in OBS.
- WebView2 without background/occlusion throttling (the StreamYard window can stay minimized).
- Thumbnails (192×108, every 2 s) only while the control room is visible.

## Usage

```bash
npm install
npm run tauri dev        # development
npm run tauri build      # NSIS installer in src-tauri/target/release/bundle
```

1. Join the StreamYard studio in the *OBSYard – StreamYard* window.
2. In the *control room* send sources to slots (1–8 buttons on the card or the slot's menus).
3. In OBS add a **Browser Source** with the slot's URL (`Copy URL`), 1920×1080, and tick *Control audio via OBS*.

Slot URL parameters: `?audio=0` (video only), `?fit=cover`, `?bg=000000` (solid background), `?debug=1` (fps/resolution/codec/latency).

The slot is **sticky by name**: if a guest disconnects and comes back, they are attached again automatically.

## Output groups

They work like guestroom.live's camera groups. A group is an ordered list of slots (fill order) plus an ordered list of guests. Guest 1 goes on the first slot, guest 2 on the second and so on. Slots left over stay empty (transparent).

- **Create** with *+ Add group* under the slots: name, slots in order and, if you like, what to do in OBS for each number of participants. The group's ⚙ edits or deletes it. A slot belongs to one group at most, and a guest to one group.
- **Use** it by sending guests to its slots (1–8 buttons on the card, the slot's menu, API):
  - a member already in the group **swaps** with whoever is on that slot;
  - a newcomer is **inserted** there: the others move down and whoever ends up past the last slot goes on **standby**, still a member;
  - an empty slot past the people present queues the guest, because the group leaves no gaps;
  - emptying a slot of the group removes whoever was on it from the group.
- Only connected guests count. If someone leaves, the others **move up at once, on air too**. Whoever comes back with the same name takes their place again. Under the slots a line shows who is *on standby* and who is *away*, with a × to remove them from the group.
- When a group is deleted, its slots keep what they show.

### OBS actions per number of participants

Connect OBS under *OBS WebSocket* (OBS → Tools → WebSocket Server Settings) and turn on **Drive OBS**. For each number of participants (from 0 to the number of slots) a group can:

- **switch to a scene** (`SetCurrentProgramScene`);
- **turn items on**, written as `Scene › Source` or `OBS group › Source`. The other items the group lists for any number are turned off (`SetSceneItemEnabled`).

That way one scene with OBS groups "1-up", "2-up", "3-up" is enough instead of a scene per layout.

The group acts only if the **program scene is one of its own**: those named in the actions, the items' containers and the *other scenes* listed. It acts when the number changes, and when OBS reaches one of its scenes while a change is waiting. Manual switches between its scenes stay untouched until the next change. It never touches streaming, recording, replay buffer or virtual cam. The group's header shows its state: in sync, OBS on another scene, items missing, OBS offline.

Groups, settings and the OBS connection are saved in `%APPDATA%\it.obsyard.app\config.json`. The obs-websocket password is saved in plain text.

### HTTP API (Stream Deck, OBS scripts)

- `GET /api/state`: full state (tracks, slots, connected OBS sources)
- `GET /api/assign?slot=1&video=Mario&audio=auto`: `video`/`audio` take a track id, part of the guest's name or `none`

### Testing without StreamYard

```bash
OBSYARD_URL=http://127.0.0.1:7878/test npm run tauri dev
```

loads a fake studio with two animated WebRTC guests.

## Known limits

- **The best quality is what StreamYard receives.** Their SFU sends simulcast and picks the layer by the size the guest is shown at in the studio: a small guest in the "backstage" can arrive at 320p. To get the high layer, keep the guest visible and large in the studio. The real resolution is shown on every card.
- Every OBS source costs one encode (in the WebView) and one decode (in OBS). With a HW encoder that is little, but one source per slot is better than duplicates.
- Guest names are taken from StreamYard's DOM heuristically: if they change the layout they may show up as "Ospite N".

## Structure

- `src-tauri/src/lib.rs`: windows, IPC commands, WebView2 arguments
- `src-tauri/src/hub.rs`: state and signaling routing
- `src-tauri/src/server.rs`: HTTP/WS server for OBS and API
- `src-inject/studio.ts`: WebRTC hooks and forwarding (injected into StreamYard)
- `src-inject/viewer.ts`: the OBS Browser Source page
- `src/`: Vue 3 + TS control room UI
