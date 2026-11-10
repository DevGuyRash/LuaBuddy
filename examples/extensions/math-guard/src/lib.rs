use luabuddy_patcher::extension::{ExtensionV1, FrameV1, MAX_PATCHES, RETURN};

unsafe extern "C" fn saturating_absolute(frame: *mut FrameV1) -> i32 {
    if frame.is_null() {
        return -1;
    }
    // SAFETY: The host provides exclusive, aligned frame storage for this call.
    let frame = unsafe { &mut *frame };
    if frame.argument_count != 1 || frame.arguments[0].tag != 1 {
        return -2;
    }
    let value = frame.arguments[0].bits as u32 as i32;
    frame.has_result = 1;
    frame.result.tag = 1;
    frame.result.bits = value.saturating_abs() as u32 as u64;
    RETURN
}

const fn identity() -> [u8; 64] {
    let bytes = b"math-guard";
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
    callbacks[0] = Some(saturating_absolute as _);
    ExtensionV1::new(identity(), 1, callbacks)
}
