//! Diagnosi senza OBS: legge un'uscita come farebbe il plugin, per qualche secondo, e stampa
//! fotogrammi al secondo, formato e audio; salva la luminanza dell'ultimo fotogramma in un PGM.
//!   cargo run --example probe -- <id uscita> [secondi] [file.pgm]

#[path = "../src/shared.rs"]
mod shared;

use obsyard_shm::{AudioHeader, FramesHeader, FRAMES_DATA};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    println!("uscite: {:?}", shared::outputs());
    let id: u32 = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(1);
    let secs: u64 = args.get(2).and_then(|a| a.parse().ok()).unwrap_or(5);
    let Some(gen) = shared::generation(id) else { return println!("uscita {id} non trovata") };
    let frames = shared::View::open(&obsyard_shm::frames_name(id, gen)).expect("memoria video");
    let audio = shared::View::open(&obsyard_shm::audio_name(id)).expect("memoria audio");
    let h = frames.get::<FramesHeader>();
    let a = audio.get::<AudioHeader>();
    let (f0, a0) = (h.frame.load(Ordering::Acquire), a.written.load(Ordering::Acquire));
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(secs) {
        h.reader_ms.store(obsyard_shm::now_ms(), Ordering::Release);
        std::thread::sleep(Duration::from_millis(100));
    }
    let el = start.elapsed().as_secs_f64();
    let (f1, a1) = (h.frame.load(Ordering::Acquire), a.written.load(Ordering::Acquire));
    let slot = h.current.load(Ordering::Acquire) as usize;
    let m = h.meta[slot];
    println!("fotogrammi: {:.1}/s  meta: {:?}", (f1 - f0) as f64 / el, m);
    println!("audio: {:.0} frame/s (atteso 48000)", (a1 - a0) as f64 / el);
    if let (Some(path), true) = (args.get(3), m.width > 0 && m.format != 0) {
        let base = unsafe { (frames.ptr() as *const u8).add(FRAMES_DATA + slot * h.slot_size as usize) };
        let (w, hh) = (m.width as usize, m.height as usize);
        let mut out = format!("P5\n{w} {hh}\n255\n").into_bytes();
        let bpp = if m.planes == 1 { 4 } else { 1 };
        for y in 0..hh {
            for x in 0..w {
                let p = unsafe { *base.add(m.offset[0] as usize + y * m.stride[0] as usize + x * bpp) };
                out.push(p);
            }
        }
        std::fs::write(path, out).unwrap();
        println!("salvato {path}");
    }
}
