//! Uscite verso il plugin OBS nativo ("OBSYard" in OBS), senza ricodifica.
//!
//! La finestra studio decodifica già ogni video: invece di ricodificarlo in WebRTC per una
//! Fonte browser, ne copia i fotogrammi (NV12/I420/RGBA, come li dà il browser) in un buffer
//! condiviso WebView2 per uscita; qui li copiamo nella memoria con nome che il plugin legge
//! (layout in ../shm-proto). Così in OBS arriva esattamente quello che il browser ha decodificato.
//!
//! - Si cattura solo un'uscita che il plugin legge (reader_ms recente nella sua memoria).
//! - Il plugin scrive anche se OBS ha la sorgente in programma o visibile: tally.
//! - Audio: anello stereo f32 a 48 kHz, stesso percorso.
//!
//! Formato dei buffer WebView2 (scritti dallo studio, letti qui), little endian:
//!   video: [0] magic "OYWV", [4] seq, [8] slot, [12] byte per slot, [16 + i·64] FrameMeta
//!          dello slot i, dati da 4096 + i·slot_size. Lo studio scrive lo slot (seq+1)%3 e poi seq.
//!   audio: [0] magic "OYWA", [4] capacità (frame), [8] frame scritti (u64), dati da 64.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::Ordering;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use obsyard_shm::{
    audio_bytes, frames_name, now_ms, put_wide, AudioHeader, FrameMeta, FramesHeader, Index, AUDIO_CAPACITY, AUDIO_CHANNELS, AUDIO_DATA,
    AUDIO_MAGIC, AUDIO_RATE, FRAMES_DATA, FRAMES_MAGIC, FRAMES_VERSION, INDEX_MAGIC, INDEX_NAME, MAX_OUTPUTS, SLOTS,
};
use serde_json::json;
use tauri::{AppHandle, Manager};

use crate::hub::SharedHub;

const WV_VIDEO_MAGIC: u32 = 0x4F59_5756; // "OYWV"
const WV_AUDIO_MAGIC: u32 = 0x4F59_5741; // "OYWA"
const WV_HEADER: usize = 4096;
/// slot iniziale: un 1080p in NV12; cresce se arriva qualcosa di più grande
const FIRST_SLOT: usize = 1920 * 1080 * 3 / 2;
/// oltre questo (4K RGBA) non cresciamo
const MAX_SLOT: usize = 3840 * 2160 * 4;
/// il plugin legge l'uscita se ha scritto negli ultimi 2 s
const READER_TIMEOUT_MS: u64 = 2000;

// ------------------------------------------------------------------ memoria con nome

#[cfg(windows)]
mod shm {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows::Win32::System::Memory::{CreateFileMappingW, MapViewOfFile, UnmapViewOfFile, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, PAGE_READWRITE};

    /// memoria condivisa con nome creata da OBSYard (il plugin la apre per nome)
    pub struct Mapping {
        handle: HANDLE,
        view: MEMORY_MAPPED_VIEW_ADDRESS,
    }

    unsafe impl Send for Mapping {}

    impl Mapping {
        pub fn create(name: &str, size: usize) -> Option<Mapping> {
            unsafe {
                let handle = CreateFileMappingW(INVALID_HANDLE_VALUE, None, PAGE_READWRITE, (size as u64 >> 32) as u32, size as u32, &HSTRING::from(name)).ok()?;
                let view = MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, size);
                if view.Value.is_null() {
                    let _ = CloseHandle(handle);
                    return None;
                }
                Some(Mapping { handle, view })
            }
        }
        pub fn ptr(&self) -> *mut u8 {
            self.view.Value as *mut u8
        }
        pub fn get<T>(&self) -> &T {
            unsafe { &*(self.view.Value as *const T) }
        }
        pub fn get_mut<T>(&mut self) -> &mut T {
            unsafe { &mut *(self.view.Value as *mut T) }
        }
    }

    impl Drop for Mapping {
        fn drop(&mut self) {
            unsafe {
                let _ = UnmapViewOfFile(self.view);
                let _ = CloseHandle(self.handle);
            }
        }
    }
}

#[cfg(windows)]
use shm::Mapping;

// ------------------------------------------------------------------ buffer WebView2

/// un buffer condiviso WebView2 vivo: il puntatore resta valido finché teniamo l'oggetto COM
struct WvBuf {
    ptr: *mut u8,
    size: usize,
    #[cfg(windows)]
    _com: webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2SharedBuffer,
}

unsafe impl Send for WvBuf {}

impl WvBuf {
    fn u32_at(&self, off: usize) -> u32 {
        unsafe { std::ptr::read_volatile(self.ptr.add(off) as *const u32) }
    }
    fn u64_at(&self, off: usize) -> u64 {
        unsafe { std::ptr::read_volatile(self.ptr.add(off) as *const u64) }
    }
    fn put_u32(&self, off: usize, v: u32) {
        unsafe { std::ptr::write_volatile(self.ptr.add(off) as *mut u32, v) }
    }
}

// ------------------------------------------------------------------ stato

struct Out {
    name: String,
    black: bool,
    gen: u32,
    #[cfg(windows)]
    frames: Option<Mapping>,
    #[cfg(windows)]
    audio: Option<Mapping>,
    video_buf: Option<WvBuf>,
    audio_buf: Option<WvBuf>,
    /// il plugin la legge: lo studio la cattura
    wanted: bool,
    /// e ne vuole anche l'audio
    wants_audio: bool,
    last_seq: u32,
    audio_read: u64,
    active: bool,
    showing: bool,
}

#[derive(Default)]
struct State {
    #[cfg(windows)]
    index: Option<Mapping>,
    outs: BTreeMap<u32, Out>,
    app: Option<AppHandle>,
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(State::default()))
}

/// stato del plugin per uscita, per la Regia: (lo legge, in programma, visibile)
pub fn status() -> HashMap<u32, (bool, bool, bool)> {
    let s = state().lock().unwrap();
    s.outs.iter().map(|(id, o)| (*id, (o.wanted, o.active, o.showing))).collect()
}

/// uscite lette dal plugin e se ne vuole l'audio (per ripassarle a una pagina ricaricata)
pub fn readers() -> Vec<(u32, bool)> {
    let s = state().lock().unwrap();
    s.outs.iter().filter(|(_, o)| o.wanted).map(|(id, o)| (*id, o.wants_audio)).collect()
}

// ------------------------------------------------------------------ avvio e uscite

/// Avvia il ciclo che copia i fotogrammi verso il plugin (uno ogni 2 ms al massimo).
pub fn start(app: AppHandle) {
    {
        let mut s = state().lock().unwrap();
        s.app = Some(app);
        #[cfg(windows)]
        {
            s.index = Mapping::create(INDEX_NAME, std::mem::size_of::<Index>());
            if let Some(m) = s.index.as_mut() {
                let idx = m.get_mut::<Index>();
                idx.magic = INDEX_MAGIC;
            }
        }
    }
    std::thread::Builder::new().name("obsyard-native".into()).spawn(run).ok();
}

/// Le uscite di OBSYard (id, nome, nero senza video): l'indice che il plugin mostra nella tendina.
pub fn set_outputs(list: &[(u32, String, bool)]) {
    let mut s = state().lock().unwrap();
    let ids: Vec<u32> = list.iter().map(|(id, _, _)| *id).collect();
    s.outs.retain(|id, _| ids.contains(id));
    for (id, name, black) in list {
        let o = s.outs.entry(*id).or_insert_with(|| Out {
            name: name.clone(),
            black: *black,
            gen: 0,
            #[cfg(windows)]
            frames: None,
            #[cfg(windows)]
            audio: None,
            video_buf: None,
            audio_buf: None,
            wanted: false,
            wants_audio: true,
            last_seq: 0,
            audio_read: 0,
            active: false,
            showing: false,
        });
        o.name = name.clone();
        o.black = *black;
        #[cfg(windows)]
        if o.frames.is_none() {
            o.gen += 1;
            o.frames = make_frames(*id, o.gen, FIRST_SLOT);
            o.audio = make_audio(*id);
        }
    }
    write_index(&mut s);
}

#[cfg(windows)]
fn make_frames(id: u32, gen: u32, slot_size: usize) -> Option<Mapping> {
    let mut m = Mapping::create(&frames_name(id, gen), FRAMES_DATA + SLOTS * slot_size)?;
    let h = m.get_mut::<FramesHeader>();
    h.magic = FRAMES_MAGIC;
    h.version = FRAMES_VERSION;
    h.slot_size = slot_size as u32;
    Some(m)
}

#[cfg(windows)]
fn make_audio(id: u32) -> Option<Mapping> {
    let mut m = Mapping::create(&obsyard_shm::audio_name(id), audio_bytes())?;
    let h = m.get_mut::<AudioHeader>();
    h.magic = AUDIO_MAGIC;
    h.rate = AUDIO_RATE;
    h.channels = AUDIO_CHANNELS;
    h.capacity = AUDIO_CAPACITY;
    Some(m)
}

fn write_index(s: &mut State) {
    #[cfg(windows)]
    {
        let list: Vec<(u32, u32, String)> = s.outs.iter().map(|(id, o)| (*id, o.gen, o.name.clone())).collect();
        let Some(m) = s.index.as_mut() else { return };
        let idx = m.get_mut::<Index>();
        let n = list.len().min(MAX_OUTPUTS);
        for (i, (id, gen, name)) in list.iter().take(n).enumerate() {
            idx.entries[i].id = *id;
            idx.entries[i].gen.store(*gen, Ordering::Release);
            put_wide(&mut idx.entries[i].name, name);
        }
        idx.count.store(n as u32, Ordering::Release);
    }
}

// ------------------------------------------------------------------ buffer verso lo studio

/// Crea (o ricrea più grande) i buffer WebView2 di un'uscita e li passa allo studio.
fn post_buffers(app: &AppHandle, id: u32, slot_size: usize) {
    #[cfg(windows)]
    {
        let Some(win) = app.get_webview_window("studio") else { return };
        let _ = win.with_webview(move |pw| {
            use webview2_com::Microsoft::Web::WebView2::Win32::{ICoreWebView2Environment12, ICoreWebView2_17, COREWEBVIEW2_SHARED_BUFFER_ACCESS_READ_WRITE};
            use windows::core::{Interface, HSTRING};
            let make = |size: usize, kind: &str, init: &dyn Fn(&WvBuf)| -> Option<WvBuf> {
                unsafe {
                    let env = pw.environment().cast::<ICoreWebView2Environment12>().ok()?;
                    let com = env.CreateSharedBuffer(size as u64).ok()?;
                    let mut ptr: *mut u8 = std::ptr::null_mut();
                    com.Buffer(&mut ptr).ok()?;
                    let buf = WvBuf { ptr, size, _com: com };
                    init(&buf);
                    let core = pw.controller().CoreWebView2().ok()?.cast::<ICoreWebView2_17>().ok()?;
                    let extra = HSTRING::from(json!({ "obsyard": kind, "slot": id }).to_string());
                    core.PostSharedBufferToScript(&buf._com, COREWEBVIEW2_SHARED_BUFFER_ACCESS_READ_WRITE, &extra).ok()?;
                    Some(buf)
                }
            };
            let video = make(WV_HEADER + SLOTS * slot_size, "video", &|b| {
                b.put_u32(0, WV_VIDEO_MAGIC);
                b.put_u32(4, 0);
                b.put_u32(8, SLOTS as u32);
                b.put_u32(12, slot_size as u32);
            });
            let audio = make(AUDIO_DATA + AUDIO_CAPACITY as usize * 2 * 4, "audio", &|b| {
                b.put_u32(0, WV_AUDIO_MAGIC);
                b.put_u32(4, AUDIO_CAPACITY);
            });
            let mut s = state().lock().unwrap();
            if let Some(o) = s.outs.get_mut(&id) {
                if video.is_none() {
                    eprintln!("[obsyard] plugin OBS: buffer video per l'uscita {id} non creato (WebView2 troppo vecchio?)");
                }
                o.video_buf = video;
                o.audio_buf = audio;
                o.last_seq = 0;
                o.audio_read = 0;
            }
        });
    }
}

/// Lo studio ha ricaricato la pagina (buffer persi): li ripassa per le uscite lette dal plugin.
pub fn repost(app: &AppHandle) {
    let wanted: Vec<(u32, usize)> = {
        let mut s = state().lock().unwrap();
        s.outs
            .iter_mut()
            .filter(|(_, o)| o.wanted)
            .map(|(id, o)| {
                o.video_buf = None;
                o.audio_buf = None;
                #[cfg(windows)]
                let size = o.frames.as_ref().map(|m| m.get::<FramesHeader>().slot_size as usize).unwrap_or(FIRST_SLOT);
                #[cfg(not(windows))]
                let size = FIRST_SLOT;
                (*id, size)
            })
            .collect()
    };
    for (id, size) in wanted {
        post_buffers(app, id, size);
    }
}

/// Lo studio ha un fotogramma che non entra nello slot: buffer (e memoria del plugin) più grandi.
pub fn grow(app: &AppHandle, id: u32, bytes: usize) {
    let size = bytes.next_power_of_two().clamp(FIRST_SLOT, MAX_SLOT);
    if bytes > MAX_SLOT {
        return;
    }
    {
        let mut s = state().lock().unwrap();
        let Some(o) = s.outs.get_mut(&id) else { return };
        #[cfg(windows)]
        {
            o.gen += 1;
            o.frames = make_frames(id, o.gen, size);
        }
        o.video_buf = None;
        write_index(&mut s);
    }
    post_buffers(app, id, size);
}

// ------------------------------------------------------------------ ciclo

fn run() {
    let mut tick: u64 = 0;
    loop {
        std::thread::sleep(Duration::from_millis(2));
        tick += 1;
        let (changed_wanted, changed_tally, app) = pass(tick);
        let Some(app) = app else { continue };
        for (id, on, audio, fresh) in &changed_wanted {
            if *on && *fresh {
                post_buffers(&app, *id, FIRST_SLOT);
            }
            if let Some(hub) = app.try_state::<SharedHub>() {
                hub.lock().unwrap().native_capture(*id, *on, *audio);
            }
        }
        if !changed_wanted.is_empty() || changed_tally {
            if let Some(hub) = app.try_state::<SharedHub>() {
                hub.lock().unwrap().emit_state();
            }
        }
    }
}

/// Un giro: battito, uscite lette dal plugin, copia dei fotogrammi e dell'audio arrivati.
/// cambi di lettura: (uscita, letta, con audio, appena cominciata)
fn pass(tick: u64) -> (Vec<(u32, bool, bool, bool)>, bool, Option<AppHandle>) {
    let mut s = state().lock().unwrap();
    let now = now_ms();
    let mut wanted_changes = vec![];
    let mut tally_changed = false;
    #[cfg(windows)]
    {
        if let Some(m) = s.index.as_ref() {
            m.get::<Index>().heartbeat_ms.store(now, Ordering::Release);
        }
        for (id, o) in s.outs.iter_mut() {
            let Some(frames) = o.frames.as_mut() else { continue };
            {
                let h = frames.get::<FramesHeader>();
                h.heartbeat_ms.store(now, Ordering::Release);
                h.empty_black.store(o.black as u32, Ordering::Relaxed);
                let reading = now.saturating_sub(h.reader_ms.load(Ordering::Acquire)) < READER_TIMEOUT_MS;
                let (active, showing) = (reading && h.reader_active.load(Ordering::Relaxed) == 1, reading && h.reader_showing.load(Ordering::Relaxed) == 1);
                let wants_audio = h.reader_audio.load(Ordering::Relaxed) == 1;
                if reading != o.wanted && (reading || tick % 50 == 0) {
                    o.wanted = reading;
                    o.wants_audio = wants_audio;
                    wanted_changes.push((*id, reading, wants_audio, reading));
                    if !reading {
                        o.video_buf = None;
                        o.audio_buf = None;
                    }
                } else if reading && wants_audio != o.wants_audio {
                    // audio acceso o spento nelle proprietà della sorgente
                    o.wants_audio = wants_audio;
                    wanted_changes.push((*id, true, wants_audio, false));
                }
                if (active, showing) != (o.active, o.showing) {
                    o.active = active;
                    o.showing = showing;
                    tally_changed = true;
                }
            }
            copy_video(o);
            copy_audio(o);
        }
    }
    (wanted_changes, tally_changed, s.app.clone())
}

/// fotogramma nuovo nel buffer dello studio -> slot libero della memoria del plugin
#[cfg(windows)]
fn copy_video(o: &mut Out) {
    let (Some(buf), Some(frames)) = (o.video_buf.as_ref(), o.frames.as_mut()) else { return };
    if buf.u32_at(0) != WV_VIDEO_MAGIC {
        return;
    }
    let seq = buf.u32_at(4);
    if seq == o.last_seq {
        return;
    }
    let wv_slot_size = buf.u32_at(12) as usize;
    let i = (seq as usize) % SLOTS;
    let meta: FrameMeta = unsafe { std::ptr::read_volatile(buf.ptr.add(16 + i * 64) as *const FrameMeta) };
    let slot_size = frames.get::<FramesHeader>().slot_size as usize;
    let size = meta.size as usize;
    if size > slot_size || size > wv_slot_size || WV_HEADER + i * wv_slot_size + size > buf.size {
        o.last_seq = seq; // non entra: lo studio chiede buffer più grandi
        return;
    }
    let h = frames.get::<FramesHeader>();
    let next = (h.current.load(Ordering::Acquire) as usize + 1) % SLOTS;
    unsafe {
        if size > 0 {
            std::ptr::copy_nonoverlapping(buf.ptr.add(WV_HEADER + i * wv_slot_size), frames.ptr().add(FRAMES_DATA + next * slot_size), size);
        }
    }
    // lo studio ha già scritto altri due fotogrammi durante la copia: questo può essere a metà
    let after = buf.u32_at(4);
    if after.wrapping_sub(seq) >= (SLOTS as u32 - 1) {
        o.last_seq = after;
        return;
    }
    let h = frames.get_mut::<FramesHeader>();
    h.meta[next] = meta;
    h.current.store(next as u32, Ordering::Release);
    h.frame.fetch_add(1, Ordering::AcqRel);
    o.last_seq = seq;
}

/// audio nuovo nell'anello dello studio -> anello del plugin
#[cfg(windows)]
fn copy_audio(o: &mut Out) {
    let (Some(buf), Some(audio)) = (o.audio_buf.as_ref(), o.audio.as_mut()) else { return };
    if buf.u32_at(0) != WV_AUDIO_MAGIC {
        return;
    }
    let cap = buf.u32_at(4) as u64;
    let written = buf.u64_at(8);
    if cap == 0 || written <= o.audio_read {
        if written < o.audio_read {
            o.audio_read = written; // lo studio ha ricominciato
        }
        audio.get::<AudioHeader>().heartbeat_ms.store(now_ms(), Ordering::Release);
        return;
    }
    let from = o.audio_read.max(written.saturating_sub(cap / 2));
    let h = audio.get::<AudioHeader>();
    let dcap = h.capacity as u64;
    let mut dw = h.written.load(Ordering::Acquire);
    let src = unsafe { buf.ptr.add(AUDIO_DATA) as *const f32 };
    let dst = unsafe { audio.ptr().add(AUDIO_DATA) as *mut f32 };
    for f in from..written {
        let s = ((f % cap) * 2) as usize;
        let d = ((dw % dcap) * 2) as usize;
        unsafe {
            *dst.add(d) = *src.add(s);
            *dst.add(d + 1) = *src.add(s + 1);
        }
        dw += 1;
    }
    let h = audio.get::<AudioHeader>();
    h.written.store(dw, Ordering::Release);
    h.heartbeat_ms.store(now_ms(), Ordering::Release);
    o.audio_read = written;
}

#[cfg(not(windows))]
fn copy_video(_: &mut Out) {}
#[cfg(not(windows))]
fn copy_audio(_: &mut Out) {}
