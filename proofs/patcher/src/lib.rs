#![deny(unsafe_op_in_unsafe_fn)]
#![allow(dead_code)]

#[path = "../../../crates/luabuddy-host/src/jvmti.rs"]
mod jvmti;
#[path = "../../../crates/luabuddy-host/src/patcher.rs"]
mod patcher;

use jni::sys::{JNI_ERR, JNI_OK, JavaVM};
use std::ffi::{c_char, c_void};

/// # Safety
/// The JVM supplies its live invocation interface. This isolated proof agent
/// must be loaded only in a disposable JVM selected by the caller.
// SAFETY: The JVM resolves this unique agent entrypoint with its documented ABI.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn Agent_OnLoad(
    vm: *mut JavaVM,
    _options: *mut c_char,
    _reserved: *mut c_void,
) -> i32 {
    let result = std::panic::catch_unwind(|| {
        if vm.is_null() {
            return Err("Null VM invocation interface".to_owned());
        }
        // SAFETY: Agent_OnLoad supplies the live VM pointer for this proof run.
        unsafe { patcher::start(vm) }
    });
    match result {
        Ok(Ok(())) => JNI_OK,
        Ok(Err(error)) => {
            eprintln!("[LuaBuddy patcher proof] initialization FAIL {error}");
            JNI_ERR
        }
        Err(_) => {
            eprintln!("[LuaBuddy patcher proof] initialization FAIL panic contained");
            JNI_ERR
        }
    }
}
