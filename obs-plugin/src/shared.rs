//! What OBSYard shares (src-tauri/src/native.rs): the layouts and names are shm-proto's.

use std::ffi::c_void;
use std::sync::atomic::Ordering;

use obsyard_shm::{wide, Index, INDEX_MAGIC, INDEX_NAME, MAX_OUTPUTS};
use windows::core::HSTRING;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Memory::{MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS};

/// a view of a named shared memory OBSYard made (read and write: the plugin writes back that
/// it reads the output and the tally)
pub struct View {
    handle: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
}

unsafe impl Send for View {}

impl View {
    pub fn open(name: &str) -> Option<View> {
        unsafe {
            let handle = OpenFileMappingW(FILE_MAP_ALL_ACCESS.0, false, &HSTRING::from(name)).ok()?;
            let view = MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, 0);
            if view.Value.is_null() {
                let _ = CloseHandle(handle);
                return None;
            }
            Some(View { handle, view })
        }
    }

    pub fn ptr(&self) -> *const c_void {
        self.view.Value
    }

    pub fn get<T>(&self) -> &T {
        unsafe { &*(self.view.Value as *const T) }
    }
}

impl Drop for View {
    fn drop(&mut self) {
        unsafe {
            let _ = UnmapViewOfFile(self.view);
            let _ = CloseHandle(self.handle);
        }
    }
}

fn index() -> Option<View> {
    let v = View::open(INDEX_NAME)?;
    let idx = v.get::<Index>();
    // OBSYard closed (its last pass is old)
    if idx.magic != INDEX_MAGIC || obsyard_shm::now_ms().saturating_sub(idx.heartbeat_ms.load(Ordering::Acquire)) > 3000 {
        return None;
    }
    Some(v)
}

/// the outputs of the running OBSYard: (id, name)
pub fn outputs() -> Vec<(u32, String)> {
    let Some(v) = index() else { return Vec::new() };
    let idx = v.get::<Index>();
    let n = (idx.count.load(Ordering::Acquire) as usize).min(MAX_OUTPUTS);
    idx.entries[..n].iter().map(|e| (e.id, wide(&e.name))).collect()
}

/// the current generation of an output's frame memory, if OBSYard has that output
pub fn generation(id: u32) -> Option<u32> {
    let v = index()?;
    let idx = v.get::<Index>();
    let n = (idx.count.load(Ordering::Acquire) as usize).min(MAX_OUTPUTS);
    idx.entries[..n].iter().find(|e| e.id == id).map(|e| e.gen.load(Ordering::Acquire))
}
