//! Guest RAM a host system call is about to read or write must be committed.
//!
//! With lazily committed guest RAM on Windows (`SMOLVM_WHP_LAZY_RAM`), a page
//! the guest has not touched, or one the balloon decommitted after the guest
//! freed it, is only reserved. A user-mode access to it faults and the VMM's
//! exception handler commits it, but a kernel-mode access does not: a
//! `recv()` or `ReadFile()` into such a buffer fails instead. Devices that hand
//! guest buffers to the host kernel call [`ensure`] on them first; the VMM
//! installs the implementation. Without lazy RAM nothing is installed and both
//! calls do nothing.

use std::sync::OnceLock;

type Hook = fn(*const u8, usize);
type DecommitHook = fn(*const u8, usize) -> bool;

static ENSURE: OnceLock<Hook> = OnceLock::new();
static DECOMMIT: OnceLock<DecommitHook> = OnceLock::new();

/// Install the VMM's implementations.
pub fn install(ensure: Hook, decommit: DecommitHook) {
    let _ = ENSURE.set(ensure);
    let _ = DECOMMIT.set(decommit);
}

/// Commit `[ptr, ptr + len)` if it lies in lazily committed guest RAM.
#[inline]
pub fn ensure(ptr: *const u8, len: usize) {
    if let Some(ensure) = ENSURE.get() {
        ensure(ptr, len);
    }
}

/// Decommit `[ptr, ptr + len)` of lazily committed guest RAM, which the guest
/// reported free. The VMM does it so the change and its record of what is
/// committed cannot race with a commit. False without lazy RAM, where a
/// decommitted page would fault the guest, or if the decommit failed.
#[inline]
pub fn decommit(ptr: *const u8, len: usize) -> bool {
    DECOMMIT.get().is_some_and(|decommit| decommit(ptr, len))
}
