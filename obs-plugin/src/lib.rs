//! OBSYard plugin for OBS Studio: the "OBSYard" source shows one OBSYard output, picture and
//! sound, taken from shared memory (shm-proto) instead of a Browser Source.
//!
//! - picture: OBSYard writes each frame as the studio window decoded it (NV12, I420 or RGBA);
//!   the source hands it to OBS as an async frame (obs_source_output_video2), OBS converts it
//!   on the graphics card. No encoding and no second decoding on the way.
//! - sound: a ring of interleaved stereo f32, handed to OBS's mixer (obs_source_output_audio)
//! - tally: the source writes back that it reads the output and whether OBS has it on program
//!   or visible, so OBSYard shows ON AIR as with a Browser Source
//! - size: the source is a fixed canvas (1920x1080 by default, as a Browser Source), so it does
//!   not change with whoever is on the output; the picture is fitted inside (transparent bands)
//!   or stretched. Frames go to a private async source ("obsyard_frames") that it draws scaled.
//!
//! Written against libobs 33 (only the stable start of obs_source_info and obs_source_frame2
//! are used, so it loads in OBS 30 and later). Linked to obs.dll by name: no OBS SDK needed.

mod shared;

use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use obsyard_shm::{format, AudioHeader, FrameMeta, FramesHeader, AUDIO_DATA, AUDIO_MAGIC, FRAMES_DATA, FRAMES_MAGIC, FRAMES_VERSION, SLOTS};
use shared::View;

type Ptr = *mut c_void;

#[repr(C)]
struct SourceInfo {
    id: *const c_char,
    kind: i32,
    output_flags: u32,
    get_name: Option<unsafe extern "C" fn(Ptr) -> *const c_char>,
    create: Option<unsafe extern "C" fn(Ptr, Ptr) -> Ptr>,
    destroy: Option<unsafe extern "C" fn(Ptr)>,
    get_width: Option<unsafe extern "C" fn(Ptr) -> u32>,
    get_height: Option<unsafe extern "C" fn(Ptr) -> u32>,
    get_defaults: Option<unsafe extern "C" fn(Ptr)>,
    get_properties: Option<unsafe extern "C" fn(Ptr) -> Ptr>,
    update: Option<unsafe extern "C" fn(Ptr, Ptr)>,
    activate: Option<unsafe extern "C" fn(Ptr)>,
    deactivate: Option<unsafe extern "C" fn(Ptr)>,
    show: Option<unsafe extern "C" fn(Ptr)>,
    hide: Option<unsafe extern "C" fn(Ptr)>,
    video_tick: Option<unsafe extern "C" fn(Ptr, f32)>,
    video_render: Option<unsafe extern "C" fn(Ptr, Ptr)>,
}

/// libobs `struct obs_source_frame2`
#[repr(C)]
struct SourceFrame2 {
    data: [*const u8; 8],
    linesize: [u32; 8],
    width: u32,
    height: u32,
    timestamp: u64,
    format: i32,
    range: i32,
    color_matrix: [f32; 16],
    color_range_min: [f32; 3],
    color_range_max: [f32; 3],
    flip: bool,
    flags: u8,
    trc: u8,
}

#[repr(C)]
struct SourceAudio {
    data: [*const u8; 8],
    frames: u32,
    speakers: i32,
    format: i32,
    samples_per_sec: u32,
    timestamp: u64,
}

const OBS_SOURCE_TYPE_INPUT: i32 = 0;
const OBS_SOURCE_VIDEO: u32 = 1 << 0;
const OBS_SOURCE_AUDIO: u32 = 1 << 1;
const OBS_SOURCE_ASYNC: u32 = 1 << 2;
const OBS_SOURCE_CUSTOM_DRAW: u32 = 1 << 3;
const OBS_SOURCE_DO_NOT_DUPLICATE: u32 = 1 << 7;
const OBS_SOURCE_CAP_DISABLED: u32 = 1 << 10;
const OBS_EFFECT_SOLID: i32 = 3;

#[repr(C, align(16))]
struct Vec4([f32; 4]);
const OBS_COMBO_TYPE_LIST: i32 = 2;
const OBS_COMBO_FORMAT_STRING: i32 = 3;
const OBS_TEXT_INFO: i32 = 3;
const VIDEO_RANGE_PARTIAL: i32 = 1;
const VIDEO_RANGE_FULL: i32 = 2;
const SPEAKERS_STEREO: i32 = 2;
const AUDIO_FORMAT_FLOAT: i32 = 4;

#[link(name = "obs", kind = "raw-dylib")]
extern "C" {
    fn obs_register_source_s(info: *const SourceInfo, size: usize);
    fn obs_data_get_string(data: Ptr, name: *const c_char) -> *const c_char;
    fn obs_data_set_default_string(data: Ptr, name: *const c_char, val: *const c_char);
    fn obs_data_get_bool(data: Ptr, name: *const c_char) -> bool;
    fn obs_data_set_default_bool(data: Ptr, name: *const c_char, val: bool);
    fn obs_properties_add_bool(props: Ptr, name: *const c_char, desc: *const c_char) -> Ptr;
    fn obs_source_create_private(id: *const c_char, name: *const c_char, settings: Ptr) -> Ptr;
    fn obs_source_release(source: Ptr);
    fn obs_source_add_active_child(parent: Ptr, child: Ptr) -> bool;
    fn obs_source_remove_active_child(parent: Ptr, child: Ptr);
    fn obs_source_video_render(source: Ptr);
    fn obs_get_base_effect(effect: i32) -> Ptr;
    fn gs_effect_get_param_by_name(effect: Ptr, name: *const c_char) -> Ptr;
    fn gs_effect_set_vec4(param: Ptr, val: *const Vec4);
    fn gs_effect_loop(effect: Ptr, technique: *const c_char) -> bool;
    fn gs_draw_sprite(tex: Ptr, flip: u32, width: u32, height: u32);
    fn gs_matrix_push();
    fn gs_matrix_pop();
    fn gs_matrix_translate3f(x: f32, y: f32, z: f32);
    fn gs_matrix_scale3f(x: f32, y: f32, z: f32);
    fn obs_properties_create() -> Ptr;
    fn obs_properties_add_list(props: Ptr, name: *const c_char, desc: *const c_char, kind: i32, format: i32) -> Ptr;
    fn obs_property_list_add_string(p: Ptr, name: *const c_char, val: *const c_char) -> usize;
    fn obs_properties_add_text(props: Ptr, name: *const c_char, desc: *const c_char, kind: i32) -> Ptr;
    fn obs_source_output_video2(source: Ptr, frame: *const SourceFrame2);
    fn obs_source_output_audio(source: Ptr, audio: *const SourceAudio);
    fn obs_source_active(source: Ptr) -> bool;
    fn obs_source_showing(source: Ptr) -> bool;
    fn video_format_get_parameters_for_format(cs: i32, range: i32, format: i32, matrix: *mut f32, min: *mut f32, max: *mut f32) -> bool;
    fn os_gettime_ns() -> u64;
    fn obs_get_locale() -> *const c_char;
}

// ------------------------------------------------------------------ language

/// OBS's language is Italian (it-IT…): the properties in Italian, otherwise in English
fn italian() -> bool {
    unsafe {
        let l = obs_get_locale();
        !l.is_null() && CStr::from_ptr(l).to_bytes().starts_with(b"it")
    }
}

/// the text for OBS's language
fn tr(it: &'static CStr, en: &'static CStr) -> *const c_char {
    if italian() { it.as_ptr() } else { en.as_ptr() }
}

// ------------------------------------------------------------------ module

static mut MODULE: Ptr = std::ptr::null_mut();

#[no_mangle]
pub unsafe extern "C" fn obs_module_set_pointer(module: Ptr) {
    MODULE = module;
}

#[no_mangle]
pub unsafe extern "C" fn obs_current_module() -> Ptr {
    MODULE
}

#[no_mangle]
pub extern "C" fn obs_module_ver() -> u32 {
    30 << 24
}

#[no_mangle]
pub extern "C" fn obs_module_name() -> *const c_char {
    c"OBSYard".as_ptr()
}

#[no_mangle]
pub extern "C" fn obs_module_description() -> *const c_char {
    c"Shows an OBSYard output (picture and sound) as an OBS source, with no encoding on the way".as_ptr()
}

#[no_mangle]
pub unsafe extern "C" fn obs_module_load() -> bool {
    let info = SourceInfo {
        id: c"obsyard_output".as_ptr(),
        kind: OBS_SOURCE_TYPE_INPUT,
        output_flags: OBS_SOURCE_VIDEO | OBS_SOURCE_AUDIO | OBS_SOURCE_CUSTOM_DRAW | OBS_SOURCE_DO_NOT_DUPLICATE,
        get_name: Some(get_name),
        create: Some(create),
        destroy: Some(destroy),
        get_width: Some(get_width),
        get_height: Some(get_height),
        get_defaults: Some(get_defaults),
        get_properties: Some(get_properties),
        update: Some(update),
        activate: None,
        deactivate: None,
        show: None,
        hide: None,
        video_tick: None,
        video_render: Some(video_render),
    };
    obs_register_source_s(&info, std::mem::size_of::<SourceInfo>());
    // the frames, as they arrive (private: hidden from the Add menu, drawn by the source above)
    let frames = SourceInfo {
        id: c"obsyard_frames".as_ptr(),
        kind: OBS_SOURCE_TYPE_INPUT,
        output_flags: OBS_SOURCE_VIDEO | OBS_SOURCE_ASYNC | OBS_SOURCE_CAP_DISABLED,
        get_name: Some(get_name),
        create: Some(frames_create),
        destroy: Some(frames_destroy),
        get_width: None,
        get_height: None,
        get_defaults: None,
        get_properties: None,
        update: None,
        activate: None,
        deactivate: None,
        show: None,
        hide: None,
        video_tick: None,
        video_render: None,
    };
    obs_register_source_s(&frames, std::mem::size_of::<SourceInfo>());
    true
}

// ------------------------------------------------------------------ source

/// what the worker and the graphics thread share
struct Shared {
    /// the output (OBSYard id) followed; 0 = none chosen
    output: AtomicU32,
    /// canvas: 0x0 = as the picture
    width: AtomicU32,
    height: AtomicU32,
    /// 0 fit inside (transparent bands), 1 stretch
    fit: AtomicU32,
    /// size of the picture on air; 0 = none
    pic_w: AtomicU32,
    pic_h: AtomicU32,
    /// without a picture: black instead of transparent
    black: AtomicBool,
    /// receive the output's sound (off: OBSYard does not even capture it)
    audio: AtomicBool,
}

struct Source {
    parent: Ptr,
    child: Ptr,
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

struct SendPtr(Ptr);
unsafe impl Send for SendPtr {}

fn setting(settings: Ptr, key: &CStr) -> String {
    unsafe {
        let v = obs_data_get_string(settings, key.as_ptr());
        if v.is_null() {
            String::new()
        } else {
            CStr::from_ptr(v).to_string_lossy().into_owned()
        }
    }
}

unsafe extern "C" fn get_name(_: Ptr) -> *const c_char {
    c"OBSYard".as_ptr()
}

unsafe extern "C" fn get_defaults(settings: Ptr) {
    obs_data_set_default_string(settings, c"output".as_ptr(), c"".as_ptr());
    obs_data_set_default_string(settings, c"resolution".as_ptr(), c"1920x1080".as_ptr());
    obs_data_set_default_string(settings, c"fit".as_ptr(), c"contain".as_ptr());
    obs_data_set_default_bool(settings, c"audio".as_ptr(), true);
}

/// settings -> what the worker and the drawing follow
fn apply(shared: &Shared, settings: Ptr) {
    shared.output.store(setting(settings, c"output").parse().unwrap_or(0), Ordering::Relaxed);
    let (w, h) = setting(settings, c"resolution")
        .split_once('x')
        .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
        .unwrap_or((0u32, 0u32));
    shared.width.store(w, Ordering::Relaxed);
    shared.height.store(h, Ordering::Relaxed);
    shared.fit.store((setting(settings, c"fit") == "stretch") as u32, Ordering::Relaxed);
    shared.audio.store(unsafe { obs_data_get_bool(settings, c"audio".as_ptr()) }, Ordering::Relaxed);
}

unsafe extern "C" fn create(settings: Ptr, obs: Ptr) -> Ptr {
    let shared = Arc::new(Shared {
        output: AtomicU32::new(0),
        width: AtomicU32::new(1920),
        height: AtomicU32::new(1080),
        fit: AtomicU32::new(0),
        pic_w: AtomicU32::new(0),
        pic_h: AtomicU32::new(0),
        black: AtomicBool::new(false),
        audio: AtomicBool::new(true),
    });
    apply(&shared, settings);
    let child = obs_source_create_private(c"obsyard_frames".as_ptr(), c"OBSYard (fotogrammi)".as_ptr(), std::ptr::null_mut());
    if !child.is_null() {
        obs_source_add_active_child(obs, child);
    }
    let stop = Arc::new(AtomicBool::new(false));
    let worker = {
        let (shared, stop, parent, frames) = (shared.clone(), stop.clone(), SendPtr(obs), SendPtr(child));
        thread::Builder::new().name("obsyard-source".into()).spawn(move || run(parent, frames, shared, stop)).ok()
    };
    Box::into_raw(Box::new(Source { parent: obs, child, shared, stop, worker })) as Ptr
}

unsafe extern "C" fn destroy(data: Ptr) {
    let mut s = Box::from_raw(data as *mut Source);
    s.stop.store(true, Ordering::Relaxed);
    if let Some(t) = s.worker.take() {
        let _ = t.join();
    }
    if !s.child.is_null() {
        obs_source_remove_active_child(s.parent, s.child);
        obs_source_release(s.child);
    }
}

unsafe extern "C" fn update(data: Ptr, settings: Ptr) {
    let s = &*(data as *const Source);
    apply(&s.shared, settings);
}

/// the canvas: the chosen size, or the picture's ("as the source"; 1920x1080 until one arrives)
fn canvas(sh: &Shared) -> (u32, u32) {
    let (w, h) = (sh.width.load(Ordering::Relaxed), sh.height.load(Ordering::Relaxed));
    if w > 0 && h > 0 {
        return (w, h);
    }
    let (pw, ph) = (sh.pic_w.load(Ordering::Relaxed), sh.pic_h.load(Ordering::Relaxed));
    if pw > 0 && ph > 0 {
        (pw, ph)
    } else {
        (1920, 1080)
    }
}

unsafe extern "C" fn get_width(data: Ptr) -> u32 {
    canvas(&(*(data as *const Source)).shared).0
}

unsafe extern "C" fn get_height(data: Ptr) -> u32 {
    canvas(&(*(data as *const Source)).shared).1
}

unsafe extern "C" fn video_render(data: Ptr, _effect: Ptr) {
    let s = &*(data as *const Source);
    let sh = &s.shared;
    let (cw, ch) = canvas(sh);
    let (pw, ph) = (sh.pic_w.load(Ordering::Relaxed), sh.pic_h.load(Ordering::Relaxed));
    if pw == 0 || ph == 0 || s.child.is_null() {
        if sh.black.load(Ordering::Relaxed) {
            // no picture and "black without video": the whole canvas
            let solid = obs_get_base_effect(OBS_EFFECT_SOLID);
            let color = gs_effect_get_param_by_name(solid, c"color".as_ptr());
            gs_effect_set_vec4(color, &Vec4([0.0, 0.0, 0.0, 1.0]));
            while gs_effect_loop(solid, c"Solid".as_ptr()) {
                gs_draw_sprite(std::ptr::null_mut(), 0, cw, ch);
            }
        }
        return;
    }
    let (sx, sy) = (cw as f32 / pw as f32, ch as f32 / ph as f32);
    let (sx, sy) = if sh.fit.load(Ordering::Relaxed) == 1 { (sx, sy) } else { (sx.min(sy), sx.min(sy)) };
    let (x, y) = ((cw as f32 - pw as f32 * sx) / 2.0, (ch as f32 - ph as f32 * sy) / 2.0);
    gs_matrix_push();
    gs_matrix_translate3f(x, y, 0.0);
    gs_matrix_scale3f(sx, sy, 1.0);
    obs_source_video_render(s.child);
    gs_matrix_pop();
}

unsafe extern "C" fn frames_create(_settings: Ptr, _obs: Ptr) -> Ptr {
    Box::into_raw(Box::new(0u8)) as Ptr
}

unsafe extern "C" fn frames_destroy(data: Ptr) {
    drop(Box::from_raw(data as *mut u8));
}

unsafe extern "C" fn get_properties(_: Ptr) -> Ptr {
    let props = obs_properties_create();
    let list = obs_properties_add_list(props, c"output".as_ptr(), tr(c"Uscita OBSYard", c"OBSYard output"), OBS_COMBO_TYPE_LIST, OBS_COMBO_FORMAT_STRING);
    let outputs = shared::outputs();
    if outputs.is_empty() {
        obs_property_list_add_string(list, tr(c"\xe2\x80\x94 OBSYard non \xc3\xa8 aperto \xe2\x80\x94", c"\xe2\x80\x94 OBSYard is not running \xe2\x80\x94"), c"".as_ptr());
    }
    for (id, name) in outputs {
        let (n, i) = (CString::new(name).unwrap_or_default(), CString::new(id.to_string()).unwrap_or_default());
        obs_property_list_add_string(list, n.as_ptr(), i.as_ptr());
    }
    let res = obs_properties_add_list(props, c"resolution".as_ptr(), tr(c"Risoluzione", c"Resolution"), OBS_COMBO_TYPE_LIST, OBS_COMBO_FORMAT_STRING);
    for (label, value) in [
        (c"1920\xc3\x971080".as_ptr(), c"1920x1080"),
        (c"1280\xc3\x97720".as_ptr(), c"1280x720"),
        (c"2560\xc3\x971440".as_ptr(), c"2560x1440"),
        (c"3840\xc3\x972160".as_ptr(), c"3840x2160"),
        (tr(c"1080\xc3\x971920 (verticale)", c"1080\xc3\x971920 (vertical)"), c"1080x1920"),
        (tr(c"Come la sorgente (cambia con chi entra)", c"As the source (changes with whoever is on)"), c"source"),
    ] {
        obs_property_list_add_string(res, label, value.as_ptr());
    }
    let fit = obs_properties_add_list(props, c"fit".as_ptr(), tr(c"Adattamento", c"Fit"), OBS_COMBO_TYPE_LIST, OBS_COMBO_FORMAT_STRING);
    obs_property_list_add_string(fit, tr(c"Dentro il riquadro (bande trasparenti)", c"Inside the frame (transparent bands)"), c"contain".as_ptr());
    obs_property_list_add_string(fit, tr(c"Stira a tutto il riquadro", c"Stretch to the whole frame"), c"stretch".as_ptr());
    obs_properties_add_bool(props, c"audio".as_ptr(), tr(c"Ricevi l'audio dell'uscita", c"Receive the output's audio"));
    obs_properties_add_text(
        props,
        c"info".as_ptr(),
        tr(
            c"L'immagine arriva come OBSYard la decodifica, senza ricodifica. Audio incluso: nelle Propriet\xc3\xa0 audio avanzate di OBS si regola come le altre sorgenti.",
            c"The picture arrives as OBSYard decodes it, with no re-encoding. Audio included: adjust it in OBS's Advanced Audio Properties like any other source.",
        ),
        OBS_TEXT_INFO,
    );
    props
}

// ------------------------------------------------------------------ worker

/// follows the chosen output: frames, sound and the tally written back
fn run(src: SendPtr, child: SendPtr, sh: Arc<Shared>, stop: Arc<AtomicBool>) {
    let mut frames: Option<(u32, u32, View)> = None; // (id, gen, view)
    let mut audio: Option<(u32, View)> = None;
    let mut last_frame: Option<u64> = None;
    let mut shown_empty = false;
    let clear = |sh: &Shared| {
        unsafe { obs_source_output_video2(child.0, std::ptr::null()) };
        sh.pic_w.store(0, Ordering::Relaxed);
        sh.pic_h.store(0, Ordering::Relaxed);
    };
    let mut audio_read: Option<u64> = None;
    let mut last_try = Instant::now() - Duration::from_secs(5);
    let mut abuf: Vec<f32> = Vec::new();

    while !stop.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_millis(2));
        let want = sh.output.load(Ordering::Relaxed);
        let gen = if want == 0 { None } else { shared::generation(want) };

        // OBSYard closed, output removed or its memory made again: let go and look again
        if frames.as_ref().is_some_and(|(id, g, _)| Some(*g) != gen || *id != want) {
            frames = None;
            last_frame = None;
        }
        if audio.as_ref().is_some_and(|(id, _)| *id != want || gen.is_none()) {
            audio = None;
            audio_read = None;
        }
        if gen.is_none() {
            if !shown_empty {
                clear(&sh);
                shown_empty = true;
            }
            continue;
        }
        if (frames.is_none() || audio.is_none()) && last_try.elapsed() >= Duration::from_millis(500) {
            last_try = Instant::now();
            if frames.is_none() {
                frames = View::open(&obsyard_shm::frames_name(want, gen.unwrap())).map(|v| (want, gen.unwrap(), v));
            }
            if audio.is_none() {
                audio = View::open(&obsyard_shm::audio_name(want)).map(|v| (want, v));
            }
        }

        // ---- picture
        if let Some((_, _, view)) = frames.as_ref() {
            let h = view.get::<FramesHeader>();
            if h.magic != FRAMES_MAGIC || h.version != FRAMES_VERSION {
                frames = None;
                continue;
            }
            let src_ptr = src.0;
            // prima cosa vogliamo, poi "sto leggendo": OBSYard non vede mai la lettura senza il resto
            h.reader_audio.store(sh.audio.load(Ordering::Relaxed) as u32, Ordering::Relaxed);
            h.reader_active.store(unsafe { obs_source_active(src_ptr) } as u32, Ordering::Relaxed);
            h.reader_showing.store(unsafe { obs_source_showing(src_ptr) } as u32, Ordering::Relaxed);
            h.reader_ms.store(obsyard_shm::now_ms(), Ordering::Release);
            sh.black.store(h.empty_black.load(Ordering::Relaxed) == 1, Ordering::Relaxed);
            let alive = obsyard_shm::now_ms().saturating_sub(h.heartbeat_ms.load(Ordering::Acquire)) < 2000;
            let n = h.frame.load(Ordering::Acquire);
            if !alive {
                if !shown_empty {
                    clear(&sh);
                    shown_empty = true;
                }
            } else if last_frame != Some(n) {
                last_frame = Some(n);
                let slot = (h.current.load(Ordering::Acquire) as usize).min(SLOTS - 1);
                let meta = h.meta[slot];
                if meta.format == 0 || meta.width == 0 {
                    // no picture: transparent, or black if the output asks for it (drawn by the source)
                    clear(&sh);
                    shown_empty = true;
                } else {
                    let base = unsafe { (view.ptr() as *const u8).add(FRAMES_DATA + slot * h.slot_size as usize) };
                    if meta.size <= h.slot_size {
                        output_frame(child.0, base, &meta);
                        sh.pic_w.store(meta.width, Ordering::Relaxed);
                        sh.pic_h.store(meta.height, Ordering::Relaxed);
                        shown_empty = false;
                    }
                }
            }
        }

        // ---- sound, ~20 ms behind OBSYard (unless turned off in the properties)
        if !sh.audio.load(Ordering::Relaxed) {
            audio_read = None;
            continue;
        }
        if let Some((_, view)) = audio.as_ref() {
            let h = view.get::<AudioHeader>();
            if h.magic != AUDIO_MAGIC || h.channels != 2 || h.capacity == 0 {
                audio = None;
                continue;
            }
            if obsyard_shm::now_ms().saturating_sub(h.heartbeat_ms.load(Ordering::Acquire)) > 1000 {
                audio_read = None;
                continue;
            }
            let written = h.written.load(Ordering::Acquire);
            let cap = h.capacity as u64;
            let lag = 960u64;
            let mut pos = match audio_read {
                Some(r) if r <= written && written - r < cap / 2 => r,
                _ => written.saturating_sub(lag),
            };
            let data = unsafe { (view.ptr() as *const u8).add(AUDIO_DATA) as *const f32 };
            while pos < written {
                let n = (written - pos).min(1024);
                abuf.clear();
                for k in 0..n {
                    let at = (((pos + k) % cap) * 2) as usize;
                    unsafe {
                        abuf.push(*data.add(at));
                        abuf.push(*data.add(at + 1));
                    }
                }
                let behind_ns = (written - pos) * 1_000_000_000 / h.rate as u64;
                let mut planes = [std::ptr::null::<u8>(); 8];
                planes[0] = abuf.as_ptr() as *const u8;
                let packet = SourceAudio {
                    data: planes,
                    frames: n as u32,
                    speakers: SPEAKERS_STEREO,
                    format: AUDIO_FORMAT_FLOAT,
                    samples_per_sec: h.rate,
                    timestamp: unsafe { os_gettime_ns() }.saturating_sub(behind_ns),
                };
                unsafe { obs_source_output_audio(src.0, &packet) };
                pos += n;
            }
            audio_read = Some(pos);
        }
    }
}

/// colour parameters for a frame (OBS converts to its own space on the graphics card)
fn frame_params(meta: &FrameMeta) -> (i32, [f32; 16], [f32; 3], [f32; 3]) {
    let rgb = matches!(meta.format, format::RGBA | format::BGRA | format::BGRX);
    let range = if rgb || meta.full_range == 1 { VIDEO_RANGE_FULL } else { VIDEO_RANGE_PARTIAL };
    let cs = if meta.colorspace == 0 { obsyard_shm::colorspace::CS_709 } else { meta.colorspace };
    let (mut m, mut lo, mut hi) = ([0f32; 16], [0f32; 3], [0f32; 3]);
    unsafe { video_format_get_parameters_for_format(cs as i32, range, meta.format as i32, m.as_mut_ptr(), lo.as_mut_ptr(), hi.as_mut_ptr()) };
    (range, m, lo, hi)
}

fn output_frame(src: Ptr, base: *const u8, meta: &FrameMeta) {
    let (range, color_matrix, color_range_min, color_range_max) = frame_params(meta);
    let mut data = [std::ptr::null::<u8>(); 8];
    let mut linesize = [0u32; 8];
    for p in 0..(meta.planes as usize).min(4) {
        data[p] = unsafe { base.add(meta.offset[p] as usize) };
        linesize[p] = meta.stride[p];
    }
    let frame = SourceFrame2 {
        data,
        linesize,
        width: meta.width,
        height: meta.height,
        timestamp: unsafe { os_gettime_ns() },
        format: meta.format as i32,
        range,
        color_matrix,
        color_range_min,
        color_range_max,
        flip: false,
        flags: 0,
        trc: 0,
    };
    // OBS copies the frame into its own buffers before returning
    unsafe { obs_source_output_video2(src, &frame) };
}
