// Copyright 2026 The libkrun Authors. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Guest memory that is reserved up front and committed on first touch.
//!
//! Windows has no overcommit: committed memory is charged against the system
//! commit limit whether or not it is ever touched, so committing a region the
//! guest may never use (a virtiofs DAX window, whose file data lives in views
//! WHP maps over it) costs its full size per VM. A region registered here is
//! only reserved. WHP accepts reserved memory in `WHvMapGpaRange`; a guest
//! access to an uncommitted page exits with `MemoryAccess` (GPA mapped), and
//! [`commit_gpa`] commits the page so the instruction completes on resume. A
//! host access (anything reading or writing through the region's host address)
//! faults in user mode and is committed by a vectored exception handler.

use std::ffi::c_void;
use std::sync::{Once, RwLock};

use windows_sys::Win32::Foundation::EXCEPTION_ACCESS_VIOLATION;
use windows_sys::Win32::System::Diagnostics::Debug::{
    AddVectoredExceptionHandler, EXCEPTION_POINTERS,
};
use windows_sys::Win32::System::Memory::{MEM_COMMIT, PAGE_READWRITE, VirtualAlloc};

/// Commit granularity: one allocation-granularity block per fault keeps the
/// exit count low (16 pages per exit) without committing much the guest did
/// not ask for.
const CHUNK: u64 = 64 * 1024;

const EXCEPTION_CONTINUE_EXECUTION: i32 = -1;
const EXCEPTION_CONTINUE_SEARCH: i32 = 0;

/// Commit granularity for guest RAM (`register_ram`): a guest that touches a
/// page of RAM usually goes on to touch its neighbours, and each exit then
/// also asks the hypervisor to back the whole chunk at once
/// (`WHvAdviseGpaRange` populate), which costs far less than the guest
/// faulting in its 512 pages one by one.
const RAM_CHUNK: u64 = 2 * 1024 * 1024;

#[derive(Clone, Copy)]
struct Range {
    gpa: u64,
    host: u64,
    size: u64,
    chunk: u64,
    populate: bool,
}

type Populate = Box<dyn Fn(u64, u64) + Send + Sync>;
static POPULATE: std::sync::OnceLock<Populate> = std::sync::OnceLock::new();

/// Set how a committed chunk of guest RAM is handed to the hypervisor in bulk
/// (the partition's `WHvAdviseGpaRange` populate).
pub fn set_populate(populate: Populate) {
    let _ = POPULATE.set(populate);
}

static RANGES: RwLock<Vec<Range>> = RwLock::new(Vec::new());

/// Whether guest RAM, not only device windows, is reserved and committed on
/// first touch (`SMOLVM_WHP_LAZY_RAM=1`). Then the VM's commit charge follows
/// what the guest uses instead of its memory limit, at the price of a
/// `MemoryAccess` exit per 64 KiB the guest touches first, and freed memory
/// the balloon reports is decommitted, which returns the charge.
pub fn ram_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        matches!(
            std::env::var("SMOLVM_WHP_LAZY_RAM").as_deref(),
            Ok("1") | Ok("on") | Ok("true")
        )
    })
}
static HANDLER: Once = Once::new();

/// Register a reserved region so guest and host touches commit it on demand.
pub fn register(gpa: u64, host: *mut u8, size: usize) {
    register_with(gpa, host, size, CHUNK, false);
}

/// [`register`] for guest RAM: committed in `RAM_CHUNK`s, each populated in
/// the hypervisor as it is committed.
pub fn register_ram(gpa: u64, host: *mut u8, size: usize) {
    register_with(gpa, host, size, RAM_CHUNK, true);
}

fn register_with(gpa: u64, host: *mut u8, size: usize, chunk: u64, populate: bool) {
    HANDLER.call_once(|| {
        // SAFETY: the handler only reads the exception record and commits pages
        // inside registered, still-reserved ranges.
        unsafe { AddVectoredExceptionHandler(1, Some(on_access_violation)) };
    });
    RANGES.write().unwrap().push(Range {
        gpa,
        host: host as u64,
        size: size as u64,
        chunk,
        populate,
    });
}

/// Commit the chunk holding `start + offset` of `range`, clamped to the range.
fn commit(range: &Range, offset: u64) -> bool {
    commit_chunk(range, offset).is_some()
}

/// Commit the chunk holding `offset`; returns its (offset, length).
fn commit_chunk(range: &Range, offset: u64) -> Option<(u64, u64)> {
    let chunk_start = offset & !(range.chunk - 1);
    let len = range.chunk.min(range.size - chunk_start);
    // SAFETY: the address lies inside a reservation this process owns, and
    // committing an already-committed page is a no-op that keeps its contents.
    let addr = unsafe {
        VirtualAlloc(
            (range.host + chunk_start) as *const c_void,
            len as usize,
            MEM_COMMIT,
            PAGE_READWRITE,
        )
    };
    (!addr.is_null()).then_some((chunk_start, len))
}

/// Called on a `MemoryAccess` exit for a mapped GPA: if the GPA lies in a
/// registered region, commit its chunk and return true so the vCPU resumes and
/// retries the instruction. False means the access is not ours (MMIO).
pub fn commit_gpa(gpa: u64) -> bool {
    let ranges = RANGES.read().unwrap();
    let Some(range) = ranges.iter().find(|r| gpa >= r.gpa && gpa - r.gpa < r.size) else {
        return false;
    };
    let Some((start, len)) = commit_chunk(range, gpa - range.gpa) else {
        return false;
    };
    if range.populate
        && let Some(populate) = POPULATE.get()
    {
        populate(range.gpa + start, len);
    }
    true
}

unsafe extern "system" fn on_access_violation(info: *mut EXCEPTION_POINTERS) -> i32 {
    // SAFETY: the system passes a valid EXCEPTION_POINTERS to a vectored handler.
    let record = unsafe { &*(*info).ExceptionRecord };
    if record.ExceptionCode != EXCEPTION_ACCESS_VIOLATION || record.NumberParameters < 2 {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let addr = record.ExceptionInformation[1] as u64;
    // A writer holds the lock only while registering at startup; never block a
    // faulting thread on it.
    let Ok(ranges) = RANGES.try_read() else {
        return EXCEPTION_CONTINUE_SEARCH;
    };
    match ranges
        .iter()
        .find(|r| addr >= r.host && addr - r.host < r.size)
    {
        Some(range) if commit(range, addr - range.host) => EXCEPTION_CONTINUE_EXECUTION,
        _ => EXCEPTION_CONTINUE_SEARCH,
    }
}
