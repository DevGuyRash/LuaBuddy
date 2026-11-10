use crate::jvmti::{CallbackEnv, Jvmti};
use jni::JNIEnv;
use jni::objects::{JObject, JString};
use jni::sys::{JNIEnv as RawJni, jmethodID, jobject};
use sha2::{Digest, Sha256};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

pub(crate) type Handler = for<'a> fn(&mut JNIEnv<'a>, &str, bool) -> Result<(), String>;
static METHOD: AtomicUsize = AtomicUsize::new(0);
static HANDLER: Mutex<Option<Handler>> = Mutex::new(None);

pub(crate) fn installed() -> bool {
    METHOD.load(Ordering::Acquire) != 0
}

pub(crate) fn prepare(env: &mut JNIEnv<'_>, jvmti: &Jvmti, handler: Handler) -> Result<(), String> {
    let class = env
        .find_class("zombie/Lua/Event")
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
        return Err("unsupported game classloader; expected system application loader".to_owned());
    }
    let method = env.get_method_id(class, "trigger", "(Lse/krka/kahlua/vm/KahluaTable;Lse/krka/kahlua/integration/LuaCaller;[Ljava/lang/Object;)Z")
        .map_err(|e| e.to_string())?.into_raw();
    if METHOD.load(Ordering::SeqCst) != 0 {
        return Err("event dispatch hook already prepared".to_owned());
    }
    let bytecodes = jvmti.bytecodes(method)?;
    let fingerprint = format!("{:x}", Sha256::digest(&bytecodes));
    let layout: serde_json::Value = serde_json::from_str(include_str!("../layouts.json"))
        .map_err(|e| format!("invalid host layout manifest: {e}"))?;
    if layout["schema"] != 1 || layout["class_loader"] != "system" {
        return Err("unsupported host layout manifest schema".to_owned());
    }
    let compatible = layout["targets"].as_array().ok_or("missing host target manifest")?.iter().any(|target| {
        target["class"] == "zombie/Lua/Event" && target["method"] == "trigger"
            && target["descriptor"] == "(Lse/krka/kahlua/vm/KahluaTable;Lse/krka/kahlua/integration/LuaCaller;[Ljava/lang/Object;)Z"
            && target["bytecode_sha256"] == fingerprint
    });
    if !compatible {
        return Err(format!(
            "unsupported Event.trigger bytecode fingerprint {fingerprint}; no lifecycle hooks installed"
        ));
    }
    let mut locations = normal_returns(&bytecodes)?;
    if locations.is_empty() {
        return Err("event dispatcher has no normal boolean return".to_owned());
    }
    locations.insert(0, 0);
    let mut installed = Vec::new();
    for location in locations {
        if let Err(error) = jvmti.breakpoint(method, location) {
            let mut rollback_errors = Vec::new();
            for installed_location in installed {
                if let Err(rollback) = jvmti.clear_breakpoint(method, installed_location) {
                    rollback_errors.push(rollback);
                }
            }
            return Err(format!(
                "event breakpoint install failed: {error}; rollback errors: {rollback_errors:?}"
            ));
        }
        installed.push(location);
    }
    *HANDLER
        .lock()
        .map_err(|_| "event handler registry poisoned")? = Some(handler);
    METHOD.store(method as usize, Ordering::SeqCst);
    Ok(())
}

/// # Safety
/// Handles must be from the active JVMTI breakpoint callback.
pub(crate) unsafe fn dispatch(
    raw: CallbackEnv,
    jni: *mut RawJni,
    thread: jobject,
    method: jmethodID,
    location: i64,
) -> Result<bool, String> {
    if METHOD.load(Ordering::SeqCst) != method as usize {
        return Ok(false);
    }
    // SAFETY: The JVM owns both environments for this callback on the event thread.
    let mut env = unsafe { JNIEnv::from_raw(jni) }.map_err(|e| e.to_string())?;
    // SAFETY: raw belongs to the current JVMTI event.
    let jvmti = unsafe { Jvmti::from_raw(raw) };
    let receiver = jvmti.local_object(thread, 0)?;
    if receiver.is_null() {
        return Err("null Event.trigger receiver".to_owned());
    }
    // SAFETY: GetLocalObject returns a valid callback-local this reference.
    let event = unsafe { JObject::from_raw(receiver) };
    let name = env
        .get_field(event, "name", "Ljava/lang/String;")
        .and_then(|value| value.l())
        .map_err(|e| e.to_string())?;
    let name = JString::from(name);
    let name: String = env.get_string(&name).map_err(|e| e.to_string())?.into();
    let handler = *HANDLER
        .lock()
        .map_err(|_| "event handler registry poisoned")?;
    if let Some(handler) = handler {
        handler(&mut env, &name, location == 0)?;
    }
    Ok(true)
}

fn normal_returns(bytes: &[u8]) -> Result<Vec<i64>, String> {
    let mut result = Vec::new();
    let mut at = 0_usize;
    let word = |offset: usize| -> Result<i32, String> {
        let value: [u8; 4] = bytes
            .get(offset..offset + 4)
            .ok_or("truncated JVM switch")?
            .try_into()
            .map_err(|_| "invalid switch word")?;
        Ok(i32::from_be_bytes(value))
    };
    while at < bytes.len() {
        let opcode = bytes[at];
        let size = match opcode {
            0x00..=0x0f
            | 0x1a..=0x35
            | 0x3b..=0x83
            | 0x85..=0x98
            | 0xac..=0xb1
            | 0xbe..=0xbf
            | 0xc2..=0xc3 => 1,
            0x10 | 0x12 | 0x15..=0x19 | 0x36..=0x3a | 0xa9 | 0xbc => 2,
            0x11
            | 0x13..=0x14
            | 0x84
            | 0x99..=0xa8
            | 0xb2..=0xb8
            | 0xbb
            | 0xbd
            | 0xc0..=0xc1
            | 0xc6..=0xc7 => 3,
            0xc5 => 4,
            0xb9..=0xba | 0xc8..=0xc9 => 5,
            0xaa => {
                let aligned = (at + 4) & !3;
                let low = word(aligned + 4)?;
                let high = word(aligned + 8)?;
                let count = i64::from(high) - i64::from(low) + 1;
                if count < 0 || count as u64 > bytes.len() as u64 / 4 {
                    return Err("invalid JVM tableswitch bounds".to_owned());
                }
                aligned - at + 12 + count as usize * 4
            }
            0xab => {
                let aligned = (at + 4) & !3;
                let count = word(aligned + 4)?;
                if count < 0 || count as usize > bytes.len() / 8 {
                    return Err("invalid JVM lookupswitch bounds".to_owned());
                }
                aligned - at + 8 + count as usize * 8
            }
            0xc4 => match bytes.get(at + 1) {
                Some(0x84) => 6,
                Some(0x15..=0x19 | 0x36..=0x3a | 0xa9) => 4,
                _ => return Err("invalid JVM wide opcode".to_owned()),
            },
            _ => return Err(format!("unsupported JVM opcode {opcode:#x} at {at}")),
        };
        if opcode == 0xac {
            result.push(at as i64);
        }
        if (0xad..=0xb1).contains(&opcode) {
            return Err("event dispatcher return type changed".to_owned());
        }
        at = at.checked_add(size).ok_or("JVM bytecode offset overflow")?;
        if at > bytes.len() {
            return Err("truncated JVM bytecode instruction".to_owned());
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::normal_returns;

    #[test]
    fn return_byte_in_an_operand_is_not_a_breakpoint() {
        assert_eq!(normal_returns(&[0x10, 0xac, 0x03, 0xac]), Ok(vec![3]));
    }

    #[test]
    fn incompatible_return_and_incomplete_instructions_are_rejected() {
        for bytes in [&[0xb1][..], &[0x11, 0x00], &[0xc4, 0xff], &[0xaa], &[0xab]] {
            assert!(normal_returns(bytes).is_err(), "accepted {bytes:?}");
        }
    }
}
