#![allow(dead_code)]

#[path = "../../../crates/luabuddy-host/src/steam.rs"]
mod steam;

use jni::{InitArgsBuilder, JNIVersion, JavaVM, NativeMethod};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, AtomicUsize, Ordering};

static REQUEST_MODE: AtomicU8 = AtomicU8::new(0);
static REQUEST_COUNT: AtomicUsize = AtomicUsize::new(0);
static REQUEST_ITEM: AtomicI64 = AtomicI64::new(0);
static REQUEST_PRIORITY: AtomicU8 = AtomicU8::new(0);
static REQUEST_RECEIVER: AtomicBool = AtomicBool::new(false);
static REQUEST_FAULT: AtomicBool = AtomicBool::new(false);

/// # Safety
/// The isolated proof JVM supplies the registered `(JZ)Z` method's JNI handles.
unsafe extern "system" fn native_download(
    raw: *mut jni::sys::JNIEnv,
    receiver: jni::sys::jobject,
    item: jni::sys::jlong,
    priority: jni::sys::jboolean,
) -> jni::sys::jboolean {
    std::panic::catch_unwind(|| {
        REQUEST_COUNT.fetch_add(1, Ordering::SeqCst);
        REQUEST_ITEM.store(item, Ordering::SeqCst);
        REQUEST_PRIORITY.store(priority, Ordering::SeqCst);
        REQUEST_RECEIVER.store(!receiver.is_null(), Ordering::SeqCst);
        match REQUEST_MODE.load(Ordering::SeqCst) {
            1 => 1,
            2 => {
                // SAFETY: The registered JNI callback supplies its current thread's environment.
                let thrown = unsafe { jni::JNIEnv::from_raw(raw) }.and_then(|mut env| {
                    env.throw_new("java/lang/IllegalStateException", "Native download fixture")
                });
                if thrown.is_err() {
                    REQUEST_FAULT.store(true, Ordering::SeqCst);
                }
                0
            }
            _ => 0,
        }
    })
    .unwrap_or_else(|_| {
        REQUEST_FAULT.store(true, Ordering::SeqCst);
        0
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let game = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("Provide the installed game's root directory")?,
    );
    let path = game.join("natives/libsteam_api.so");
    let java = game.join("projectzomboid.jar");
    if !path.is_file() || !java.is_file() {
        return Err("Expected the installed Linux game JAR and Steam library".into());
    }
    // SAFETY: This isolated proof deliberately opens the user's unmodified host
    // library for symbol resolution. It calls no Steam initialization or account API.
    let library = unsafe { libloading::Library::new(&path)? };
    for (name, name_bytes) in steam::REQUIRED_EXPORTS {
        // SAFETY: Resolve an address only; no guessed function is executed.
        unsafe { library.get::<*mut std::ffi::c_void>(name_bytes) }
            .map_err(|error| format!("Missing required host export {name}: {error}"))?;
    }
    println!(
        "PASS: all {} current-host Workshop symbols exist; no Steam API was called",
        steam::REQUIRED_EXPORTS.len()
    );
    let args = InitArgsBuilder::new()
        .version(JNIVersion::V8)
        .option(format!("-Djava.class.path={}", java.display()))
        .option("-Dzomboid.steam=0")
        .option("-Xcheck:jni")
        .option("-Xverify:all")
        .build()?;
    let vm = JavaVM::new(args)?;
    let mut env = vm.attach_current_thread()?;
    let route = steam::HostDownload::bind(&mut env)?;
    if env.exception_check()? {
        return Err("Native download-route binding left a pending Java exception".into());
    }
    let class = env.find_class("zombie/core/znet/SteamWorkshop")?;
    env.register_native_methods(
        class,
        &[NativeMethod {
            name: "n_DownloadItem".into(),
            sig: "(JZ)Z".into(),
            fn_ptr: native_download as *mut std::ffi::c_void,
        }],
    )?;
    let item = steam::ItemId::new(42)?;
    if route.request(item)? {
        return Err("Native requester rejection was ignored".into());
    }
    REQUEST_MODE.store(2, Ordering::SeqCst);
    if route.request(item).is_ok() || env.exception_check()? {
        return Err("Native requester exception did not become a cleared Rust error".into());
    }
    REQUEST_MODE.store(1, Ordering::SeqCst);
    if !route.request(item)?
        || REQUEST_COUNT.load(Ordering::SeqCst) != 3
        || REQUEST_ITEM.load(Ordering::SeqCst) != 42
        || REQUEST_PRIORITY.load(Ordering::SeqCst) != 0
        || !REQUEST_RECEIVER.load(Ordering::SeqCst)
        || REQUEST_FAULT.load(Ordering::SeqCst)
    {
        return Err(
            "Native requester did not receive the exact item, priority, and receiver".into(),
        );
    }
    let foreign = std::thread::scope(|scope| {
        scope
            .spawn(|| {
                let _attached = vm
                    .attach_current_thread()
                    .map_err(|error| error.to_string())?;
                Ok::<_, String>(route.request(item))
            })
            .join()
    });
    if !matches!(foreign, Ok(Ok(Err(_)))) || REQUEST_COUNT.load(Ordering::SeqCst) != 3 {
        return Err("A foreign-thread native download request reached the host".into());
    }
    drop(route);
    println!(
        "PASS: actual system-loader Workshop instance exposes private native n_DownloadItem(JZ)Z"
    );
    println!(
        "PASS: isolated Rust native requester verifies arguments, rejection, owner thread and cleared Java exceptions; no Steam account API was called"
    );
    match steam::SteamWorkshop::attach_existing(&mut env, &path) {
        Err(error) if error.0.contains("without Steam") => {
            println!(
                "PASS: actual host SteamUtils rejects Workshop access before Steam initialization"
            );
        }
        Err(error) => return Err(format!("Unexpected unavailable-context failure: {error}").into()),
        Ok(_) => return Err("Unexpected Workshop context in an uninitialized proof JVM".into()),
    }
    println!(
        "NOT RUN: live search, subscribe, download, and in-game UI need an owned initialized game context"
    );
    Ok(())
}
