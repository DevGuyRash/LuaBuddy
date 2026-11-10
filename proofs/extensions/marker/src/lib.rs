use luabuddy_patcher::extension::{CONTINUE, ExtensionV1, FrameV1, MAX_PATCHES};

extern "C" fn initializer() {
    if let Some(path) = std::env::var_os("LUABUDDY_EXTENSION_PROOF_MARKER") {
        let _ = std::fs::write(path, b"native initializer executed");
    }
}

#[used]
// SAFETY: Linux x86_64 ELF invokes this C-ABI function at library load. Its
// only effect is the explicitly selected disposable verification marker.
#[unsafe(link_section = ".init_array")]
static INITIALIZER: extern "C" fn() = initializer;

unsafe extern "C" fn observe(_frame: *mut FrameV1) -> i32 {
    CONTINUE
}

// SAFETY: This unique symbol returns the fixed ABI v1 descriptor by value.
#[unsafe(no_mangle)]
pub extern "C" fn luabuddy_extension_v1() -> ExtensionV1 {
    let mut id = [0; 64];
    id[..16].copy_from_slice(b"marker-extension");
    let mut callbacks = [None; MAX_PATCHES];
    callbacks[0] = Some(observe as _);
    ExtensionV1::new(id, 1, callbacks)
}
