use jni::objects::{GlobalRef, JClass, JFieldID, JMethodID, JObject, JString};
use jni::signature::{Primitive, ReturnType};
use jni::sys::{JNIEnv as RawJni, jboolean, jobject, jvalue};
use jni::{JNIEnv, NativeMethod};
use sha2::{Digest, Sha256};
use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) const DESCRIPTOR: &str =
    "(Lse/krka/kahlua/vm/KahluaTable;Lse/krka/kahlua/integration/LuaCaller;[Ljava/lang/Object;)Z";
pub(crate) const ORIGINAL: &str = "luabuddy$trigger";
/// Receives a failure the handler could not report through its own result.
pub(crate) type Reporter = fn(&str);
pub(crate) type Handler = for<'a> fn(&mut JNIEnv<'a>, &str, bool) -> Result<(), String>;

struct Dispatcher {
    original: JMethodID,
    name: JFieldID,
    _class: GlobalRef,
}

static DISPATCHER: OnceLock<Dispatcher> = OnceLock::new();
static HANDLER: OnceLock<(Handler, &'static [&'static str], Reporter)> = OnceLock::new();
static REGISTERED: AtomicBool = AtomicBool::new(false);

pub(crate) fn installed() -> bool {
    registered() && HANDLER.get().is_some()
}

pub(crate) fn registered() -> bool {
    REGISTERED.load(Ordering::Acquire)
}

pub(crate) fn verify(bytecodes: &[u8]) -> Result<(), String> {
    let fingerprint = format!("{:x}", Sha256::digest(bytecodes));
    let layout: serde_json::Value = serde_json::from_str(include_str!("../layouts.json"))
        .map_err(|e| format!("invalid host layout manifest: {e}"))?;
    if layout["schema"] != 1 || layout["class_loader"] != "system" {
        return Err("unsupported host layout manifest schema".into());
    }
    let compatible = layout["targets"]
        .as_array()
        .ok_or("missing host target manifest")?
        .iter()
        .any(|target| {
            target["class"] == "zombie/Lua/Event"
                && target["method"] == "trigger"
                && target["descriptor"] == DESCRIPTOR
                && target["bytecode_sha256"] == fingerprint
        });
    if !compatible {
        return Err(format!(
            "unsupported Event.trigger bytecode fingerprint {fingerprint}; no lifecycle hooks installed"
        ));
    }
    Ok(())
}

/// Called only for the transformed Event class in the approved system loader.
pub(crate) fn register(env: &mut JNIEnv<'_>, class: &JClass<'_>) -> Result<(), String> {
    let dispatcher = Dispatcher {
        original: env
            .get_method_id(class, ORIGINAL, DESCRIPTOR)
            .map_err(|e| e.to_string())?,
        name: env
            .get_field_id(class, "name", "Ljava/lang/String;")
            .map_err(|e| e.to_string())?,
        _class: env.new_global_ref(class).map_err(|e| e.to_string())?,
    };
    // Publish valid IDs before RegisterNatives can expose the entrypoint. The
    // global class reference keeps both IDs valid through JVM shutdown.
    DISPATCHER
        .set(dispatcher)
        .map_err(|_| "event bridge already registered")?;
    env.register_native_methods(
        class,
        &[NativeMethod {
            name: "trigger".into(),
            sig: DESCRIPTOR.into(),
            fn_ptr: trigger as *mut c_void,
        }],
    )
    .map_err(|e| e.to_string())?;
    REGISTERED.store(true, Ordering::Release);
    Ok(())
}

/// Observes only the named events. Every event the game triggers passes the
/// bridge, so other names are filtered before anything is decoded.
pub(crate) fn prepare(
    env: &mut JNIEnv<'_>,
    handler: Handler,
    names: &'static [&'static str],
    report: Reporter,
) -> Result<(), String> {
    env.find_class("zombie/Lua/Event")
        .map_err(|e| e.to_string())?;
    if !registered() {
        return Err("native event dispatcher was not registered".into());
    }
    HANDLER
        .set((handler, names, report))
        .map_err(|_| "event dispatch hook already prepared".into())
}

/// The event's name when anything here could care about it. Every event the
/// game triggers passes through, so names of other lengths are never decoded.
fn watched(env: &mut JNIEnv<'_>, event: &JObject<'_>, dispatcher: &Dispatcher) -> Option<String> {
    let (_, names, _) = HANDLER.get()?;
    let name = env
        .get_field_unchecked(event, dispatcher.name, ReturnType::Object)
        .and_then(|value| value.l())
        .ok()?;
    let name = env.auto_local(JString::from(name));
    let length = string_length(env, &name)?;
    // Java counts a string's length in UTF-16 units.
    if !names
        .iter()
        .any(|watched| watched.encode_utf16().count() == length)
    {
        return None;
    }
    let text = env.get_string(&name).ok()?;
    let text: std::borrow::Cow<'_, str> = (&*text).into();
    names.contains(&&*text).then(|| text.into_owned())
}

fn string_length(env: &JNIEnv<'_>, text: &JString<'_>) -> Option<usize> {
    if text.is_null() {
        return None;
    }
    let raw = env.get_raw();
    // SAFETY: The environment is the current thread's, and the string is a live,
    // non-null local reference. GetStringLength runs no Java code.
    let length = unsafe { ((**raw).GetStringLength?)(raw, text.as_raw()) };
    usize::try_from(length).ok()
}

fn observe(env: &mut JNIEnv<'_>, name: &str, enter: bool) {
    let Some((handler, _, report)) = HANDLER.get() else {
        return;
    };
    let outcome =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handler(env, name, enter)));
    match outcome {
        Ok(Ok(())) => (),
        error => {
            let error = match error {
                Ok(Err(error)) => error,
                _ => "a panic was contained".to_owned(),
            };
            if matches!(env.exception_check(), Ok(true)) {
                let _ = env.exception_describe();
                let _ = env.exception_clear();
            }
            report(&format!("LuaBuddy could not handle {name}: {error}"));
        }
    }
}

unsafe extern "system" fn trigger(
    jni: *mut RawJni,
    receiver: jobject,
    table: jobject,
    caller: jobject,
    arguments: jobject,
) -> jboolean {
    let outcome = std::panic::catch_unwind(|| -> Result<jboolean, String> {
        // SAFETY: RegisterNatives binds exactly DESCRIPTOR on the final Event
        // class. The JVM supplies thread-local JNI and this non-null receiver.
        let mut env = unsafe { JNIEnv::from_raw(jni) }.map_err(|e| e.to_string())?;
        // SAFETY: The receiver is a live local reference for this native call.
        let event = unsafe { JObject::from_raw(receiver) };
        let dispatcher = DISPATCHER.get().ok_or("event bridge unavailable")?;
        let name = watched(&mut env, &event, dispatcher);
        if let Some(name) = &name {
            observe(&mut env, name, true);
        }
        // SAFETY: The original private method is resolved on the same final
        // class and pinned by _class. Its unchanged descriptor accepts these
        // three JVM-supplied reference arguments and returns boolean. No lock
        // is held while game code or nested events execute.
        let result = unsafe {
            env.call_method_unchecked(
                &event,
                dispatcher.original,
                ReturnType::Primitive(Primitive::Boolean),
                &[
                    jvalue { l: table },
                    jvalue { l: caller },
                    jvalue { l: arguments },
                ],
            )
        };
        // Preserve the game's pending exception and omit the normal-return
        // observation, exactly as the former return breakpoints did.
        let value = result
            .map_err(|e| e.to_string())?
            .z()
            .map_err(|e| e.to_string())?;
        if let Some(name) = &name {
            observe(&mut env, name, false);
        }
        Ok(jboolean::from(value))
    });
    match outcome {
        Ok(Ok(value)) => value,
        error => {
            // SAFETY: The native call still owns this JNI environment. Never
            // replace or clear an exception raised by the original dispatcher.
            if let Ok(env) = unsafe { JNIEnv::from_raw(jni) }
                && !matches!(env.exception_check(), Ok(true))
            {
                eprintln!("[LuaBuddy] event dispatch failed: {error:?}");
            }
            0
        }
    }
}
