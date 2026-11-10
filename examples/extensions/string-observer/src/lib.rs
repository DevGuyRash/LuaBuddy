use luabuddy_patcher::extension::{CONTINUE, ExtensionV1, FrameV1, MAX_PATCHES};
use std::sync::atomic::{AtomicU64, Ordering};

static CALLS: AtomicU64 = AtomicU64::new(0);

unsafe extern "C" fn observe(frame: *mut FrameV1) -> i32 {
    if frame.is_null() {
        return -1;
    }
    CALLS.fetch_add(1, Ordering::Relaxed);
    CONTINUE
}

const fn identity() -> [u8; 64] {
    let bytes = b"string-observer";
    let mut output = [0; 64];
    let mut index = 0;
    while index < bytes.len() {
        output[index] = bytes[index];
        index += 1
    }
    output
}

// SAFETY: This unique symbol implements the published ABI v1 entrypoint.
#[unsafe(no_mangle)]
pub extern "C" fn luabuddy_extension_v1() -> ExtensionV1 {
    let mut callbacks = [None; MAX_PATCHES];
    callbacks[0] = Some(observe as _);
    ExtensionV1::new(identity(), 1, callbacks)
}

// SAFETY: The exported diagnostic returns a scalar and retains no host memory.
#[unsafe(no_mangle)]
pub extern "C" fn luabuddy_observed_calls_v1() -> u64 {
    CALLS.load(Ordering::Relaxed)
}
