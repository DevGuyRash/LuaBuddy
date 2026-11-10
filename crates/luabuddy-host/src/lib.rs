//! Project Zomboid's Rust-owned runtime and JVM boundary.

mod activation;
mod agent;
mod bridge;
mod callables;
mod classfile;
mod download_events;
mod events;
mod extensions;
mod game;
#[path = "../../../profiles/hooks.rs"]
mod hooks;
mod jvmti;
mod lua;
pub mod patcher;
mod proof;
mod report;
mod runtime;
pub mod steam;
mod ui;

use jni::sys::{JNI_ERR, JNI_OK, JavaVM};
use std::ffi::{CStr, c_char, c_void};

/// Starts the runtime or an explicitly requested isolated integration proof.
///
/// # Safety
/// The JVM supplies live invocation pointers and a NUL-terminated agent option.
// SAFETY: The JVM looks up this exact agent symbol and supplies its JNI ABI.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn Agent_OnLoad(
    vm: *mut JavaVM,
    options: *mut c_char,
    _reserved: *mut c_void,
) -> i32 {
    let outcome = std::panic::catch_unwind(|| {
        if vm.is_null() {
            return Err("JVM invocation interface is null".to_owned());
        }
        // SAFETY: Agent_OnLoad's caller guarantees a NUL-terminated option string.
        let option = if options.is_null() {
            b"".as_slice()
        } else {
            unsafe { CStr::from_ptr(options) }.to_bytes()
        };
        if matches!(option, b"" | b"runtime") {
            // SAFETY: The JVM supplies this invocation pointer for agent startup.
            if let Err(error) = unsafe { agent::start(vm) } {
                eprintln!("[LuaBuddy] Rust runtime inactive: {error}");
            }
            return Ok(());
        }
        if !matches!(option, b"proof" | b"proof-native" | b"proof-patcher") {
            return Err("unknown native agent operation".to_owned());
        }
        // SAFETY: The VM invocation pointer is valid for the agent's lifetime.
        if option == b"proof-patcher" {
            // SAFETY: The VM pointer belongs to this live agent initialization.
            unsafe { patcher::start(vm) }
        } else {
            // SAFETY: The VM pointer belongs to this live agent initialization.
            unsafe { proof::start(vm, option == b"proof-native") }
        }
    });
    match outcome {
        Ok(Ok(())) => JNI_OK,
        Ok(Err(error)) => {
            eprintln!("[LuaBuddy native proof] initialization failed: {error}");
            JNI_ERR
        }
        Err(_) => {
            eprintln!("[LuaBuddy native proof] panic contained at agent entry");
            JNI_ERR
        }
    }
}
