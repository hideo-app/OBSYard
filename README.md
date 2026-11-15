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
- Thumbnails (192×108) only while the control room is visible, at 0.5–15 fps (*Previews* menu at the top; one every 2 s by default).

## Usage

```bash
npm install
npm run tauri dev        # development
npm run tauri build      # NSIS installer in src-tauri/target/release/bundle
```

1. Join the StreamYard studio in the *OBSYard – StreamYard* window.
2. In the *control room* send sources to outputs: 1…N buttons on the card, drag and drop, Shift+1…9 or the output's menus (see *Sending sources to outputs*).
3. In OBS add a **Browser Source** with the output's URL (`Copy URL`), 1920×1080, and tick *Control audio via OBS*.

URL parameters: `?audio=0` (video only), `?fit=cover`, `?bg=000000` (solid background), `?lt=0` (no lower third on that source), `?debug=1` (fps/resolution/codec/latency).

### Sources

- The control room shows **only who is on air** in StreamYard, with their StreamYard name. Whoever goes to the backstage disappears, and their output stays empty until they are back on air.
- Names and on-air state come from StreamYard's protocol (`streamUpdated`, `clientList`, `clientUpdated`), not from the page's text.
- Your camera is forwarded **as captured**, before StreamYard's colour filters.
- An output is **sticky by name**: if a guest disconnects and comes back, they are attached again automatically.

### OBS outputs

Like guestroom.live's outputs:

- **+ Add output** under the list suggests the next *Cam N* name.
- Each output's ⋮ menu has **Rename** (also by double-clicking the name), **Move output up/down** (also Alt+↑/↓ with the row focused), the **connection details**, that output's lower third, the *Without video* background and **Delete**.
- The URL uses a stable id (`/slot/{id}`): renaming or moving does not touch the sources in OBS.
- A source can also open the output **by name**: `/slot/Cam%201` (⋮ → *Copy URL by name*; case and spaces do not matter). The name is read when OBS opens the page: after renaming the output, that source has to be updated. If a name is also another output's id (e.g. an output called "2"), the id wins.
- A deleted output also leaves its group. A group left without outputs disappears.

**Without video** (empty output, guest off air, camera off, guest stuck with no frame for more than 1.2 s), the output does not freeze on the last frame.

- It shows what you pick in **Without video: Transparent / Black**, at the top of the outputs.
- Each output can differ from its ⋮ menu (*As the setting*, *Transparent* or *Black*); the exception shows as a tag next to the name.

### OBS plugin (no re-encoding)

Instead of a Browser Source, OBSYard has its own OBS plugin (Windows, OBS 30 or later): the **OBSYard** source.

- **Install**: control room → *OBS plugin* → **Install the plugin** (then restart OBS). It goes into `%ProgramData%\obs-studio\plugins\obsyard-obs`, with no admin rights. When OBSYard is updated the section says *needs update*: close OBS and press **Update the plugin**.
- **Use**: in OBS, **+ → OBSYard**, and pick the output by name from the list.
- **No re-encoding**: the studio window passes the frames exactly as the browser decoded them (NV12, I420 or RGBA), and OBS converts them on the graphics card. A Browser Source instead receives video re-encoded as H.264. Quality, delay and encoder load all gain. The transfer happens in shared memory (WebView2 shared buffers → named memory → plugin), modelled on CueRider's plugin; the layouts are in `shm-proto`.
- **Fixed resolution**: the source is a fixed-size frame, 1920×1080 by default, that does not change with whoever is on. The picture fits inside with transparent bands, or is *stretched*. There are also 1280×720, 2560×1440, 3840×2160, 1080×1920 vertical and *As the source*.
- **Audio**: reaches OBS's mixer like any other source, captured on an AudioWorklet (48 kHz stereo). Turning off **Receive the output's audio** in the properties also stops OBSYard from capturing it.
- **Tally**: the source tells OBSYard whether it is on program or visible, so ON AIR works as with a Browser Source. In the control room, outputs read by the plugin carry a green **plugin** tag.
- **Only what OBS reads is captured**: an output with no OBSYard source costs nothing.
- **Limits**: lower thirds *on the output* are drawn by the Browser Source's page, so with the plugin use the ones *in OBS Text sources*. The plugin and OBSYard must run on the same PC.
- **Diagnostics**: `cargo run --release --example probe -- <output id> [seconds] [file.pgm]` in `obs-plugin` reads an output like the plugin and prints frames per second, format and audio.

### Sending sources to outputs

Outputs are numbered 1…N in the order they appear on screen: the same number as the buttons on the cards.

- **Buttons on the card**: button N sends the source to output N; pressed again it removes it.
- **Drag and drop**: drag a card onto an output. While you drag, the output under the pointer says what dropping will do (*Send here*, *Replace*, *Insert here*).
- **Shift+1…9**: click a card to select it (blue border), then Shift+N sends it to output N. Esc deselects.
- **Swapping outputs**: drag an output's thumbnail onto another. Two outputs outside groups swap what they show; two outputs of the same group swap the people's places; otherwise the first one's source is sent to the second.

### Tally (ON AIR)

Every Browser Source tells OBSYard whether it is **on program** in OBS or only **visible** (preview in Studio Mode), through OBS's `obsSourceActiveChanged` and `obsSourceVisibleChanged` events.

- The output and the card of the source it shows turn red with **ON AIR**; green with **PREVIEW** when they are only visible. At the top the control room counts the outputs on program.
- Neither obs-websocket nor *Drive OBS* is needed.
- OBS tells a source only when the state **changes**, never when the page loads. After a source refresh, or if OBS starts on that scene already, the tally shows up at the first scene change.
- Guests do not get it: they are in StreamYard.

### Connection details

An output's **OBS N** button (or ⋮ → *Show connection details*) opens a panel under the row, refreshed every second: connection state, resolution and fps received by OBS, bitrate, codec, delay (jitter buffer and RTT), lost packets and frames, freezes, audio. With several OBS sources on the same output there is one per source. The data is read only while the panel is open.

### Lower thirds

Optional, chosen in the control room's **Lower thirds** section: *Off*, *On the output* or *In OBS Text sources*.

- The text is the person's StreamYard name. The **Aa** button on the card changes the name shown and adds a **second line** (role, handle…). It stays saved for that name and applies every time the person comes back on air.
- An output is left out from its ⋮ menu (*Lower third on this output*), a single source with `?lt=0` in the URL.
- The lower third shows only while the output has video.

**On the output**: the output's page draws the lower third over the video. Restyle it in the Browser Source's *Custom CSS* field, with the same classes and variables as guestroom.live:

| Hook | What it is |
| --- | --- |
| `.lower-third` | the bar (bottom left) |
| `.lower-third-name`, `.lower-third-title` | the name and the second line (absent when empty) |
| `data-source-kind="camera"` / `"screen"` | on `.lower-third`: the kind of source |
| `data-state="in"` / `"out"` | on `.lower-third`: comes in with `in`, goes back to `out` before leaving |
| `--lt-bg`, `--lt-fg`, `--lt-accent` | background, text and accent bar |
| `--lt-font`, `--lt-name-size`, `--lt-title-size` | font and sizes (4.4% and 2.6% of the height) |
| `--lt-offset-x`, `--lt-offset-y`, `--lt-radius` | distance from the edges and corner radius |

The default rules have zero specificity, so a plain selector is enough to change them.

**In OBS Text sources**: OBSYard writes name and second line into two Text sources per output, called `<output> nome` and `<output> titolo` (for example `Cam 1 nome`), and empties them when the output empties or lower thirds are turned off. The pages draw nothing.

- It needs obs-websocket: OBSYard connects even without *Drive OBS*.
- If the sources are missing, the output shows *text missing* and the Lower thirds section offers **Create in OBS**: it creates them at the bottom left of the current scene (the preview scene in Studio Mode). Then move and style them in OBS.
- Renaming an output also changes the names of the sources OBSYard looks for.

### Guests at full resolution

StreamYard lets each guest decide their resolution: the height of their tile in the layout × the broadcast resolution. Measured with a 720p broadcast:

| Layout | Resolution sent |
|---|---|
| backstage | 427×240 |
| group / thin | 960×540 |
| full-height tile | 1280×720 |

In the control room turn on **Guests at full resolution**: OBSYard prepares the layout, selects it and remembers the previous one. Turning it off goes back to the previous one. The switch's state is saved from one start to the next.

- **The "OBSYard" layout is defined by OBSYard**: one full-height column per camera the plan can have on air (StreamYard tells the number when you join the studio). Only the height counts, so every guest sends at their maximum.
- If the layout is missing, or someone edited it, OBSYard creates it or **overwrites** it through the same API as StreamYard's editor (`/api/workspaces/{ws}/layouts`). It uses fixed ids and writes only when something differs.
- The studio uses the copy of the layouts it read when it opened. After a write the switch shows **"Layout updated: reload the studio"**: press *Reload*, and as soon as you are back the layout turns on by itself. From then on the switch changes layout instantly.
- The layout is selected by clicking StreamYard's own button, without injecting messages into the protocol.
- If you change layout by hand while it is on, it does not force it and says so.
- The layout's name is changed under *Forwarding quality*.

It also changes StreamYard's broadcast layout: it makes sense if you direct the show in OBS.

### Debug

`OBSYARD_CDP_PORT=9222` opens WebView2's remote debugging. `OBSYARD_URL=…` opens another page in the studio, for example `http://127.0.0.1:7878/test`.

For testing and development use `OBSYARD_CONFIG_DIR=<folder>`: the configuration goes there instead of `%APPDATA%\it.obsyard.app`. That way you do not touch the installed app's, which uses the same folder in the development build too.

In the studio, `localStorage.obsyardTap = "1"` records StreamYard's WebSocket messages and HTTP requests in `window.__obsyardTap`, useful if StreamYard changes protocol.

## Output groups

On first start OBSYard creates four outputs and a group, *Gruppo 1*, holding all of them. It can be edited or deleted like the others.

They work like guestroom.live's camera groups. A group is an ordered list of slots (fill order) plus an ordered list of guests. Guest 1 goes on the first slot, guest 2 on the second and so on. Slots left over stay empty (transparent).

- **Create** with *+ Add group* under the outputs: name, outputs in order and, if you like, what to do in OBS for each number of participants. The group's ⚙ edits or deletes it. A slot belongs to one group at most, and a guest to one group.
- **Use** it by sending guests to its outputs (buttons on the card, the output's menu, API):
  - a member already in the group **swaps** with whoever is on that slot;
  - a newcomer is **inserted** there: the others move down and whoever ends up past the last slot goes on **standby**, still a member;
  - the first free output takes the guest at once. A free output **further on** asks first, as in guestroom, because the group leaves no gaps. You can cancel, or **fill up to there** with that person (fill test, below). From the API, with no question, the guest is queued;
  - emptying a slot of the group removes whoever was on it from the group.
- Only connected guests count. If someone leaves, the others **move up at once, on air too**. Whoever comes back with the same name takes their place again. Under the slots a line shows who is *on standby* and who is *away*, with a × to remove them from the group.
- **Reorder** members by dragging a group output onto another (swapping places), with *Person up/down* in the ⋮ menu or Alt+↑/↓ on the row. Someone on standby moves up one place with their ↑, or is dragged onto a group output. Those away stay where they are.
- When a group is deleted, its slots keep what they show.

### Fill test (same person on several outputs)

It works like guestroom's "test fill", to try on your own scenes meant for several people.

- Sending someone to a group output past the first free one, the control room asks whether to **fill up to there**. Confirming, that person goes on that output and on every free output before it. People already on air stay where they are.
- During the test, outputs past the people on air take **anyone** you send, without asking: the same person on several outputs, or different people in non-consecutive places.
- OBS actions count every filled output.
- Under the group a **yellow notice** says who is there and up to where, with a × to end the test. The test's outputs carry the *test* tag.
- The test also ends when you empty one of its outputs or when the group's members change.
- Outside groups the same source can go on as many outputs as you like.

### Duplicates (same source on several outputs)

- Outside groups, just send the source to another output.
- On a **free** output of a group, if the source is already on another output, the control room asks: **Duplicate** (it also stays where it was), **Move** (the group's rules) or **Cancel**.
- A duplicate does not change the group's order and counts as a filled output for OBS actions. It goes away when the output is emptied, given another source, when a group member lands there or when the person leaves.
- Every output showing a source that is also elsewhere carries a yellow **duplicate** tag, with the other outputs in its tooltip.

### Auto-add and priority

They are set in the control room's **Participants** section and apply to the whole app.

- **Add automatically to the first group**: whoever goes on air (guests and your camera, not shared screens) and is in no group joins the **first group**, the one highest in the outputs, in order of arrival.
  - It fills the outputs in sequence, and whoever goes past the last one ends up *on standby*. The group shows the **auto** badge.
  - Whoever you remove by hand (× or emptied output) is not added again. They come back only if you send them to an output yourself, or if they leave and rejoin the studio.
- **Priority**: a list of names, one per line, the first being the most important.
  - Whoever is on the list joins the group ahead of those with less priority or not on the list. If the outputs are full, the last one goes on standby.
  - It also applies when a guest's name arrives a moment after their track.
  - "Mario" also matches "Mario Rossi"; case and spaces do not matter.
  - When you edit the list, members are reordered once. Moves you make by hand afterwards stay, until you press **Reorder now** under the list: it puts every group back in priority order.

### OBS actions per number of participants

Connect OBS under *OBS WebSocket* (OBS → Tools → WebSocket Server Settings) and turn on **Drive OBS**. OBSYard connects to OBS only with *Drive OBS* on (or with lower thirds in Text sources). For each number of participants (from 0 to the number of slots) a group can:

- **switch to a scene** (`SetCurrentProgramScene`);
- **turn items on**, written as `Scene › Source` or `OBS group › Source`. The other items the group lists for any number are turned off (`SetSceneItemEnabled`).

That way one scene with OBS groups "1-up", "2-up", "3-up" is enough instead of a scene per layout.

The group acts only if the **program scene is one of its own**: those named in the actions, the items' containers and the *other scenes* listed. It acts when the number changes, and when OBS reaches one of its scenes while a change is waiting. Manual switches between its scenes stay untouched until the next change. It never touches streaming, recording, replay buffer or virtual cam. The group's header shows its state: in sync, OBS on another scene, items missing, OBS offline.

Groups, settings and the OBS connection are saved in `%APPDATA%\it.obsyard.app\config.json`. The obs-websocket password is saved in plain text.

#### Scene sets

Like guestroom.live's "scene sets": besides the main scenes a group can have up to four **scene sets** (*+ Add scene set* in the group editor), each with a name, its own actions per number of participants and its own *other scenes*. For example the camera scenes "Cam 1", "Cam 2", "Cam 3" in the main set and a "Reaction" set with "Reaction 1", "Reaction 2", "Reaction 3", with the cameras in picture-in-picture over the content.

- The set holding the **program scene** acts: on "Reaction 2" a third participant leads to "Reaction 3", never to the camera scenes.
- When you move to a scene of another set, that set's state for the current number applies **once**: on "Cam 1" with two participants OBS goes to "Cam 2". Moving between the scenes of the same set stays free.
- A scene, including the containers of the items a set turns on, belongs to **one set** at most: the editor and saving refuse otherwise.
- Each set turns off only the items it lists itself. The group's header shows the set in use.

### HTTP API (Stream Deck, OBS scripts)

- `GET /api/state`: full state (tracks, slots, connected OBS sources)
- `GET /api/assign?slot=1&video=Mario&audio=auto`: `slot` takes the output's id or name (`slot=Cam%201`); `video`/`audio` take a track id, part of the guest's name or `none`

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
