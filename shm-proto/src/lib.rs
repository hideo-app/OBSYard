//! What OBSYard shares with its OBS plugin through named shared memory (the same idea as
//! CueRider's shm-proto): the `#[repr(C)]` layouts, their magics and names. One copy for the
//! app (src-tauri) and the plugin (obs-plugin): a layout changed here changes for both.
//!
//! - `Local\OBSYard-index`: the app's outputs (id, name, generation of their frame memory)
//! - `Local\OBSYard-out-<id>-<gen>`: one output's frames, already decoded (no encoding on the
//!   way to OBS); the plugin writes back that it reads them and whether OBS has them on air
//! - `Local\OBSYard-out-<id>-audio`: one output's sound, interleaved stereo f32

use std::sync::atomic::{AtomicU32, AtomicU64};

/// a millisecond clock both processes agree on (heartbeats)
#[cfg(windows)]
pub fn now_ms() -> u64 {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetTickCount64() -> u64;
    }
    unsafe { GetTickCount64() }
}

#[cfg(not(windows))]
pub fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// `s` into a 0-terminated UTF-16 field (cut to fit)
pub fn put_wide(dst: &mut [u16], s: &str) {
    let n = dst.len() - 1;
    let mut i = 0;
    for c in s.encode_utf16().take(n) {
        dst[i] = c;
        i += 1;
    }
    dst[i..].iter_mut().for_each(|c| *c = 0);
}

/// a 0-terminated UTF-16 field as a string
pub fn wide(s: &[u16]) -> String {
    let n = s.iter().position(|c| *c == 0).unwrap_or(s.len());
    String::from_utf16_lossy(&s[..n])
}

// ------------------------------------------------------------------ index

/// "OYIX"
pub const INDEX_MAGIC: u32 = 0x4F59_4958;
pub const MAX_OUTPUTS: usize = 64;
pub const INDEX_NAME: &str = r"Local\OBSYard-index";

#[repr(C)]
pub struct IndexEntry {
    pub id: u32,
    /// generation of the output's frame memory: it is made again (bigger) when a frame no
    /// longer fits, under a new name
    pub gen: AtomicU32,
    pub name: [u16; 64],
}

#[repr(C)]
pub struct Index {
    pub magic: u32,
    pub count: AtomicU32,
    /// `now_ms` of the app's last pass: the plugin shows nothing when it gets old
    pub heartbeat_ms: AtomicU64,
    pub entries: [IndexEntry; MAX_OUTPUTS],
}

// ------------------------------------------------------------------ frames

/// "OYFR"
pub const FRAMES_MAGIC: u32 = 0x4F59_4652;
pub const FRAMES_VERSION: u32 = 1;
/// frames in the ring: the app writes one while the plugin reads another
pub const SLOTS: usize = 3;
/// bytes before the first slot
pub const FRAMES_DATA: usize = 4096;

/// OBS's `enum video_format` values used here
pub mod format {
    pub const I420: u32 = 1;
    pub const NV12: u32 = 2;
    pub const RGBA: u32 = 6;
    pub const BGRA: u32 = 7;
    pub const BGRX: u32 = 8;
    pub const I444: u32 = 10;
}

/// OBS's `enum video_colorspace` values used here
pub mod colorspace {
    pub const CS_601: u32 = 1;
    pub const CS_709: u32 = 2;
    pub const SRGB: u32 = 3;
}

/// one frame's description (one per slot)
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct FrameMeta {
    pub width: u32,
    pub height: u32,
    /// `format::*`; 0 = no picture (empty output, camera off)
    pub format: u32,
    /// `colorspace::*`
    pub colorspace: u32,
    pub full_range: u32,
    pub planes: u32,
    /// bytes from the slot's start, and bytes per line, of each plane
    pub offset: [u32; 4],
    pub stride: [u32; 4],
    /// bytes used in the slot
    pub size: u32,
    pub _pad: u32,
}

#[repr(C)]
pub struct FramesHeader {
    pub magic: u32,
    pub version: u32,
    /// bytes per slot
    pub slot_size: u32,
    /// slot holding the latest complete frame
    pub current: AtomicU32,
    /// frames written so far (the plugin outputs a new one when this moves)
    pub frame: AtomicU64,
    /// `now_ms` of the app's last pass over this output
    pub heartbeat_ms: AtomicU64,
    pub meta: [FrameMeta; SLOTS],
    /// what to show without a picture: 0 transparent, 1 black
    pub empty_black: AtomicU32,
    // ---- written by the plugin
    /// `now_ms` of the plugin's last read: the app only captures outputs someone reads
    pub reader_ms: AtomicU64,
    /// the OBS source is on program (1) / visible somewhere, e.g. preview (1)
    pub reader_active: AtomicU32,
    pub reader_showing: AtomicU32,
    /// the source receives the sound (1) or has it turned off in its properties (0)
    pub reader_audio: AtomicU32,
}

pub fn frames_name(id: u32, gen: u32) -> String {
    format!(r"Local\OBSYard-out-{id}-{gen}")
}

// ------------------------------------------------------------------ sound

/// "OYAU"
pub const AUDIO_MAGIC: u32 = 0x4F59_4155;
pub const AUDIO_RATE: u32 = 48_000;
pub const AUDIO_CHANNELS: u32 = 2;
/// frames in the ring (a power of two): ~0.7 s
pub const AUDIO_CAPACITY: u32 = 32_768;
pub const AUDIO_DATA: usize = 64;

#[repr(C)]
pub struct AudioHeader {
    pub magic: u32,
    pub rate: u32,
    pub channels: u32,
    pub capacity: u32,
    /// frames written so far (position = written % capacity)
    pub written: AtomicU64,
    pub heartbeat_ms: AtomicU64,
}

pub fn audio_name(id: u32) -> String {
    format!(r"Local\OBSYard-out-{id}-audio")
}

pub const fn audio_bytes() -> usize {
    AUDIO_DATA + AUDIO_CAPACITY as usize * AUDIO_CHANNELS as usize * 4
}

// the sizes both sides were built with: a mismatch does not compile
const _: () = assert!(std::mem::size_of::<IndexEntry>() == 136);
const _: () = assert!(std::mem::size_of::<Index>() == 16 + 64 * 136);
const _: () = assert!(std::mem::size_of::<FrameMeta>() == 64);
const _: () = assert!(std::mem::size_of::<AudioHeader>() == 32);
const _: () = assert!(std::mem::size_of::<FramesHeader>() <= FRAMES_DATA);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_fields_round_trip_and_cut() {
        let mut f = [7u16; 6];
        put_wide(&mut f, "Cam 1");
        assert_eq!(wide(&f), "Cam 1");
        put_wide(&mut f, "Cam 12345");
        assert_eq!(wide(&f), "Cam 1");
        assert_eq!(f[5], 0);
    }
}
