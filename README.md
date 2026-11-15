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
2. In the *control room* send sources to outputs (1…N buttons on the card, in the outputs' order, or the output's menus).
3. In OBS add a **Browser Source** with the output's URL (`Copy URL`), 1920×1080, and tick *Control audio via OBS*.

URL parameters: `?audio=0` (video only), `?fit=cover`, `?bg=000000` (solid background), `?debug=1` (fps/resolution/codec/latency).

### Sources

- The control room shows **only who is on air** in StreamYard, with their StreamYard name. Whoever goes to the backstage disappears, and their output stays empty until they are back on air.
- Names and on-air state come from StreamYard's protocol (`streamUpdated`, `clientList`, `clientUpdated`), not from the page's text.
- Your camera is forwarded **as captured**, before StreamYard's colour filters.
- An output is **sticky by name**: if a guest disconnects and comes back, they are attached again automatically.

### OBS outputs

Like guestroom.live's outputs:

- **+ Add output** under the list suggests the next *Cam N* name.
- Each output's ⋮ menu has **Rename** (also by double-clicking the name), **Move up/down**, that output's *Without video* background and **Delete**.
- The URL uses a stable id (`/slot/{id}`): renaming or moving does not touch the sources in OBS.
- A deleted output also leaves its group. A group left without outputs disappears.

**Without video** (empty output, guest off air, camera off, guest stuck with no frame for more than 1.2 s), the output does not freeze on the last frame.

- It shows what you pick in **Without video: Transparent / Black**, at the top of the outputs.
- Each output can differ from its ⋮ menu (*As the setting*, *Transparent* or *Black*); the exception shows as a tag next to the name.

### Guests at full resolution

StreamYard lets each guest decide their resolution: the height of their tile in the layout × the broadcast resolution. Measured with a 720p broadcast:

| Layout | Resolution sent |
|---|---|
| backstage | 427×240 |
| group / thin | 960×540 |
| full-height tile | 1280×720 |

In the control room turn on **Guests at full resolution**: OBSYard prepares the layout, selects it and remembers the previous one. Turning it off goes back to the previous one.

- **The "OBSYard" layout is defined by OBSYard**: one full-height column per camera the plan can have on air (StreamYard tells the number when you join the studio). Only the height counts, so every guest sends at their maximum.
- If the layout is missing, or someone edited it, OBSYard creates it or **overwrites** it through the same API as StreamYard's editor (`/api/workspaces/{ws}/layouts`). It uses fixed ids and writes only when something differs.
- The studio uses the copy of the layouts it read when it opened. After a write the switch shows **"Layout updated: reload the studio"**: press *Reload*, and as soon as you are back the layout turns on by itself. From then on the switch changes layout instantly.
- The layout is selected by clicking StreamYard's own button, without injecting messages into the protocol.
- If you change layout by hand while it is on, it does not force it and says so.
- The layout's name is changed under *Forwarding quality*.

It also changes StreamYard's broadcast layout: it makes sense if you direct the show in OBS.

### Debug

`OBSYARD_CDP_PORT=9222` opens WebView2's remote debugging. `OBSYARD_URL=…` opens another page in the studio, for example `http://127.0.0.1:7878/test`.

In the studio, `localStorage.obsyardTap = "1"` records StreamYard's WebSocket messages and HTTP requests in `window.__obsyardTap`, useful if StreamYard changes protocol.

## Output groups

They work like guestroom.live's camera groups. A group is an ordered list of slots (fill order) plus an ordered list of guests. Guest 1 goes on the first slot, guest 2 on the second and so on. Slots left over stay empty (transparent).

- **Create** with *+ Add group* under the outputs: name, outputs in order and, if you like, what to do in OBS for each number of participants. The group's ⚙ edits or deletes it. A slot belongs to one group at most, and a guest to one group.
- **Use** it by sending guests to its outputs (buttons on the card, the output's menu, API):
  - a member already in the group **swaps** with whoever is on that slot;
  - a newcomer is **inserted** there: the others move down and whoever ends up past the last slot goes on **standby**, still a member;
  - an empty slot past the people present queues the guest, because the group leaves no gaps;
  - emptying a slot of the group removes whoever was on it from the group.
- Only connected guests count. If someone leaves, the others **move up at once, on air too**. Whoever comes back with the same name takes their place again. Under the slots a line shows who is *on standby* and who is *away*, with a × to remove them from the group.
- When a group is deleted, its slots keep what they show.

### Auto-add and priority

They are set in the control room's **Participants** section and apply to the whole app.

- **Add automatically to the first group**: whoever goes on air (guests and your camera, not shared screens) and is in no group joins the **first group**, the one highest in the outputs, in order of arrival.
  - It fills the outputs in sequence, and whoever goes past the last one ends up *on standby*. The group shows the **auto** badge.
  - Whoever you remove by hand (× or emptied output) is not added again. They come back only if you send them to an output yourself, or if they leave and rejoin the studio.
- **Priority**: a list of names, one per line, the first being the most important.
  - Whoever is on the list joins the group ahead of those with less priority or not on the list. If the outputs are full, the last one goes on standby.
  - It also applies when a guest's name arrives a moment after their track.
  - "Mario" also matches "Mario Rossi"; case and spaces do not matter.
  - When you edit the list, members are reordered once. Moves you make by hand afterwards stay.

### OBS actions per number of participants

Connect OBS under *OBS WebSocket* (OBS → Tools → WebSocket Server Settings) and turn on **Drive OBS**. OBSYard connects to OBS only with *Drive OBS* on. For each number of participants (from 0 to the number of slots) a group can:

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
