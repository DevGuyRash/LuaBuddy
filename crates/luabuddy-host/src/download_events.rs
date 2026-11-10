use crate::{
    jvmti::{CallbackEnv, Jvmti},
    steam,
};
use jni::{
    JNIEnv,
    sys::{jmethodID, jobject},
};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicUsize, Ordering};

static SUCCESS: AtomicUsize = AtomicUsize::new(0);
static FAILURE: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn installed() -> bool {
    SUCCESS.load(Ordering::Acquire) != 0 && FAILURE.load(Ordering::Acquire) != 0
}

pub(crate) fn prepare(env: &mut JNIEnv<'_>, jvmti: &Jvmti) -> Result<(), String> {
    let class = env
        .find_class("zombie/core/znet/SteamWorkshop")
        .map_err(|e| e.to_string())?;
    let loader = env
        .call_method(&class, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
        .and_then(|v| v.l())
        .map_err(|e| e.to_string())?;
    let system = env
        .call_static_method(
            "java/lang/ClassLoader",
            "getSystemClassLoader",
            "()Ljava/lang/ClassLoader;",
            &[],
        )
        .and_then(|v| v.l())
        .map_err(|e| e.to_string())?;
    if !env
        .is_same_object(loader, system)
        .map_err(|e| e.to_string())?
    {
        return Err("Unapproved Workshop callback classloader".into());
    }
    let layout: serde_json::Value =
        serde_json::from_str(include_str!("../layouts.json")).map_err(|e| e.to_string())?;
    let mut methods = Vec::new();
    for (name, descriptor) in [
        ("onItemDownloaded", "(J)V"),
        ("onItemNotDownloaded", "(JI)V"),
    ] {
        let method = env
            .get_method_id(&class, name, descriptor)
            .map_err(|e| e.to_string())?
            .into_raw();
        let digest = format!("{:x}", Sha256::digest(jvmti.bytecodes(method)?));
        if !layout["targets"]
            .as_array()
            .ok_or("Missing callback compatibility manifest")?
            .iter()
            .any(|target| {
                target["class"] == "zombie/core/znet/SteamWorkshop"
                    && target["method"] == name
                    && target["descriptor"] == descriptor
                    && target["bytecode_sha256"] == digest
            })
        {
            return Err(format!(
                "Unsupported Workshop callback {name} fingerprint {digest}"
            ));
        }
        methods.push(method);
    }
    jvmti.breakpoint(methods[0], 0)?;
    if let Err(error) = jvmti.breakpoint(methods[1], 0) {
        return match jvmti.clear_breakpoint(methods[0], 0) {
            Ok(()) => Err(error),
            Err(rollback) => Err(format!("{error}; callback rollback uncertain: {rollback}")),
        };
    }
    SUCCESS.store(methods[0] as usize, Ordering::Release);
    FAILURE.store(methods[1] as usize, Ordering::Release);
    steam::download_observer_ready();
    Ok(())
}

/// # Safety
/// Handles belong to the current breakpoint event, with locals suspended at method entry.
pub(crate) unsafe fn dispatch(
    raw: CallbackEnv,
    thread: jobject,
    method: jmethodID,
) -> Result<bool, String> {
    let success = SUCCESS.load(Ordering::Acquire) == method as usize;
    if !success && FAILURE.load(Ordering::Acquire) != method as usize {
        return Ok(false);
    }
    // SAFETY: The agent supplies the active callback environment and matching method argument layout.
    let jvmti = unsafe { Jvmti::from_raw(raw) };
    let item =
        steam::ItemId::new(jvmti.local_long(thread, 1)? as u64).map_err(|e| e.to_string())?;
    let result = if success {
        1
    } else {
        jvmti.local_int(thread, 3)?
    };
    steam::record_download_result(item, result).map_err(|e| e.to_string())?;
    Ok(true)
}
