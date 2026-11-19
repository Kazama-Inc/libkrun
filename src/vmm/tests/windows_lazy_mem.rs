//! `lazy_mem`: what is marked committed must be committed, whatever the
//! balloon decommits at the same time.
#![cfg(target_os = "windows")]

use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use krun_vmm::lazy_mem::{decommit, ensure_committed, register_ram};
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_RESERVE, MEMORY_BASIC_INFORMATION, PAGE_READWRITE, VirtualAlloc, VirtualQuery,
};

const RAM_CHUNK: usize = 2 * 1024 * 1024;

fn is_committed(addr: u64, len: u64) -> bool {
    let end = addr + len;
    let mut at = addr;
    while at < end {
        let mut mbi: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: VirtualQuery only reads the address space.
        let n = unsafe { VirtualQuery(at as *const c_void, &mut mbi, std::mem::size_of_val(&mbi)) };
        if n == 0 || mbi.State != MEM_COMMIT {
            return false;
        }
        at = mbi.BaseAddress as u64 + mbi.RegionSize as u64;
    }
    true
}

/// The guest frees pages (the balloon decommits them) and later uses them
/// again as I/O buffers, while pages next to them in the same chunk are
/// being freed. A buffer `ensure_committed` has returned for must be
/// committed, or the host kernel's I/O into it fails.
#[test]
fn ensured_buffers_are_committed_while_neighbours_are_decommitted() {
    const SLOT: usize = 16 * 1024;
    const SIZE: usize = 4 * RAM_CHUNK;
    const SLOTS: usize = SIZE / SLOT;
    // SAFETY: a fresh reservation this test owns for the process lifetime.
    let host = unsafe { VirtualAlloc(std::ptr::null(), SIZE, MEM_RESERVE, PAGE_READWRITE) };
    assert!(!host.is_null());
    let host = host as u64;
    register_ram(0x7f00_0000_0000, host as *mut u8, SIZE);

    // A slot is owned by one thread at a time, as a page is either free
    // (the balloon may decommit it) or in use (a device may do I/O on it).
    let owned: Arc<Vec<AtomicBool>> =
        Arc::new((0..SLOTS).map(|_| AtomicBool::new(false)).collect());
    let stop = Arc::new(AtomicBool::new(false));
    let broken = Arc::new(AtomicU64::new(0));
    let take = |owned: &[AtomicBool], i: usize| {
        owned[i]
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    };

    let mut threads = Vec::new();
    for t in 0..4u64 {
        let (owned, stop, broken) = (owned.clone(), stop.clone(), broken.clone());
        threads.push(std::thread::spawn(move || {
            let mut x = 0x9e37_79b9_7f4a_7c15u64 ^ t;
            while !stop.load(Ordering::Relaxed) {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                let i = (x % SLOTS as u64) as usize;
                if !take(&owned, i) {
                    continue;
                }
                let addr = host + (i * SLOT) as u64;
                if t == 0 {
                    decommit(addr as *const u8, SLOT);
                } else {
                    ensure_committed(addr as *const u8, SLOT);
                    if !is_committed(addr, SLOT as u64) {
                        broken.fetch_add(1, Ordering::Relaxed);
                    }
                }
                owned[i].store(false, Ordering::Release);
            }
        }));
    }
    std::thread::sleep(std::time::Duration::from_secs(3));
    stop.store(true, Ordering::Relaxed);
    for t in threads {
        t.join().unwrap();
    }
    assert_eq!(
        broken.load(Ordering::Relaxed),
        0,
        "ensured buffers left uncommitted"
    );
}

/// Guest memory that is not lazily committed RAM (the kernel image region is
/// committed up front) must not be decommitted: nothing would commit it again.
#[test]
fn memory_outside_registered_ram_is_not_decommitted() {
    const SIZE: usize = 64 * 1024;
    // SAFETY: a fresh committed allocation this test owns.
    let host = unsafe {
        VirtualAlloc(
            std::ptr::null(),
            SIZE,
            MEM_COMMIT | MEM_RESERVE,
            PAGE_READWRITE,
        )
    } as u64;
    assert_ne!(host, 0);
    assert!(!decommit(host as *const u8, SIZE));
    assert!(is_committed(host, SIZE as u64));
}
