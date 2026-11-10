use super::{JvmPatcher, jni_error};
use jni::JNIEnv;
use luabuddy_patcher::extension::LoadedExtension;
use luabuddy_patcher::{
    Error, HookControl, HookKind, Invocation, PatchSpec, Value, bytecode_sha256,
};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub(super) fn run(env: &mut JNIEnv<'_>) -> std::result::Result<(), Box<dyn std::error::Error>> {
    use jni::objects::{JString, JValue};
    use luabuddy_patcher::Reentrancy;
    use luabuddy_patcher::extension::{EnableConsent, ExtensionManifest, Privilege, verify};
    use std::path::PathBuf;

    let property = |env: &mut JNIEnv<'_>,
                    name: &str|
     -> std::result::Result<PathBuf, Box<dyn std::error::Error>> {
        let key = env.new_string(name)?;
        let value = env
            .call_static_method(
                "java/lang/System",
                "getProperty",
                "(Ljava/lang/String;)Ljava/lang/String;",
                &[JValue::Object(&key)],
            )?
            .l()?;
        if value.is_null() {
            return Err(format!("Supply -D{name}=<repository-built extension library>").into());
        }
        let string = JString::from(value);
        Ok(PathBuf::from(
            env.get_string(&string)?.to_string_lossy().into_owned(),
        ))
    };
    let math_path = property(env, "luabuddy.proof.math")?;
    let observer_path = property(env, "luabuddy.proof.observer")?;
    let baseline = env
        .call_static_method("java/lang/Math", "abs", "(I)I", &[JValue::Int(i32::MIN)])?
        .i()?;
    if baseline != i32::MIN {
        return Err("The JVM's baseline Math.abs behavior differs from the Java contract".into());
    }
    let baseline_empty = env.new_string("")?;
    if !env
        .call_static_method(
            "zombie/util/StringUtils",
            "isNullOrEmpty",
            "(Ljava/lang/String;)Z",
            &[JValue::Object(&baseline_empty)],
        )?
        .z()?
    {
        return Err(
            "The game string predicate baseline differs from the empty-string contract".into(),
        );
    }
    if env
        .call_static_method(
            "java/lang/Integer",
            "sum",
            "(II)I",
            &[JValue::Int(2), JValue::Int(3)],
        )?
        .i()?
        != 5
    {
        return Err("The JVM integer sum baseline differs from 2+3=5".into());
    }
    let math = JvmPatcher::target(env, "java/lang/Math", "abs", "(I)I", "bootstrap")?;
    let string = JvmPatcher::target(
        env,
        "zombie/util/StringUtils",
        "isNullOrEmpty",
        "(Ljava/lang/String;)Z",
        "system",
    )?;
    let compare = JvmPatcher::target(env, "java/lang/Integer", "compare", "(II)I", "bootstrap")?;
    let sum = JvmPatcher::target(env, "java/lang/Integer", "sum", "(II)I", "bootstrap")?;
    if !matches!(
        JvmPatcher::target(env, "java/lang/Math", "abs", "(I)I", "system"),
        Err(Error::StaleTarget(_))
    ) {
        return Err("An incorrect classloader identity was accepted".into());
    }
    if !matches!(
        JvmPatcher::target(env, "java/lang/Math", "abs", "(J)J", "bootstrap"),
        Err(Error::Unsupported(_))
    ) {
        return Err("An unsupported wide argument layout was accepted".into());
    }
    let mut patcher = JvmPatcher::default();
    for (path, id, target, kind, privilege) in [
        (
            &math_path,
            "math-guard",
            math.clone(),
            HookKind::Replace,
            Privilege::ReplaceMethods,
        ),
        (
            &observer_path,
            "string-observer",
            string.clone(),
            HookKind::Before,
            Privilege::ObserveMethods,
        ),
    ] {
        let digest = bytecode_sha256(&std::fs::read(path)?);
        let privileges = BTreeSet::from([privilege]);
        let manifest = ExtensionManifest {
            schema_version: 1,
            abi_version: 1,
            id: id.into(),
            artifact_sha256: digest.clone(),
            provenance: "repository-built Rust extension example".into(),
            target_os: std::env::consts::OS.into(),
            target_arch: std::env::consts::ARCH.into(),
            privileges: privileges.clone(),
            patches: vec![PatchSpec {
                id: format!("{id}/live"),
                owner: id.into(),
                target,
                kind,
                requires: vec![],
                priority: 0,
                reentrancy: Reentrancy::SkipSelf,
            }],
        };
        let consent = EnableConsent {
            extension_id: id.into(),
            artifact_sha256: digest,
            provenance: manifest.provenance.clone(),
            allowed_privileges: privileges,
            install: true,
            enable: true,
        };
        let verified = verify(path, manifest, &consent)?;
        // SAFETY: The explicit disposable proof consumes only the two reviewed
        // Rust extension examples built by its runner, with digest-bound intent.
        patcher.register_extension(unsafe { LoadedExtension::load(verified)? })?;
    }

    let vm = Arc::new(env.get_java_vm()?);
    let calls = Arc::new(AtomicU64::new(0));
    let nested_calls = Arc::clone(&calls);
    let nested_vm = Arc::clone(&vm);
    patcher.register(
        PatchSpec {
            id: "proof/compare".into(),
            owner: "proof".into(),
            target: compare.clone(),
            kind: HookKind::Before,
            requires: vec![],
            priority: 0,
            reentrancy: Reentrancy::SkipSelf,
        },
        Arc::new(move |invocation: &mut Invocation| {
            nested_calls.fetch_add(1, Ordering::SeqCst);
            if invocation.arguments.first() == Some(&Value::Int(13)) {
                let mut env = nested_vm.get_env().map_err(jni_error)?;
                let result = env
                    .call_static_method(
                        "java/lang/Integer",
                        "compare",
                        "(II)I",
                        &[JValue::Int(13), JValue::Int(21)],
                    )
                    .and_then(|v| v.i())
                    .map_err(jni_error)?;
                if result != -1 {
                    return Err(Error::Hook(
                        "Nested JVM call changed its native result".into(),
                    ));
                }
            }
            Ok(HookControl::Continue)
        }),
    )?;
    patcher.register(
        PatchSpec {
            id: "proof/sum".into(),
            owner: "proof".into(),
            target: sum.clone(),
            kind: HookKind::ModifyArguments,
            requires: vec![],
            priority: 0,
            reentrancy: Reentrancy::SkipSelf,
        },
        Arc::new(|invocation: &mut Invocation| {
            invocation.arguments[0] = Value::Int(7);
            Ok(HookControl::Continue)
        }),
    )?;
    let argument_order = Arc::new(Mutex::new(Vec::new()));
    for (id, priority) in [("proof/sum-before", -10), ("proof/sum-after", 10)] {
        let argument_order = Arc::clone(&argument_order);
        patcher.register(
            PatchSpec {
                id: id.into(),
                owner: "proof".into(),
                target: sum.clone(),
                kind: HookKind::Before,
                requires: vec![],
                priority,
                reentrancy: Reentrancy::SkipSelf,
            },
            Arc::new(move |invocation: &mut Invocation| {
                let Value::Int(value) = invocation.arguments[0] else {
                    return Err(Error::Hook("Expected integer sum argument".into()));
                };
                argument_order
                    .lock()
                    .map_err(|_| Error::Hook("Proof observation lock poisoned".into()))?
                    .push(value);
                Ok(HookControl::Continue)
            }),
        )?;
    }
    let installed = patcher.activate(
        env,
        &[
            "math-guard/live".into(),
            "string-observer/live".into(),
            "proof/compare".into(),
            "proof/sum".into(),
            "proof/sum-before".into(),
            "proof/sum-after".into(),
        ],
    )?;
    if installed.len() != 6 {
        return Err("Not every live patch received a native installation receipt".into());
    }
    let guarded = env
        .call_static_method("java/lang/Math", "abs", "(I)I", &[JValue::Int(i32::MIN)])?
        .i()?;
    if guarded != i32::MAX {
        return Err(format!("Rust math extension did not replace Math.abs: {guarded}").into());
    }
    let empty = env.new_string("")?;
    let result = env
        .call_static_method(
            "zombie/util/StringUtils",
            "isNullOrEmpty",
            "(Ljava/lang/String;)Z",
            &[JValue::Object(&empty)],
        )?
        .z()?;
    if !result {
        return Err("The observer changed the game's original string predicate".into());
    }
    // SAFETY: This is the reviewed example library with a scalar diagnostic ABI.
    let observer = unsafe { libloading::Library::new(&observer_path)? };
    // SAFETY: The reviewed example exports exactly this scalar function signature.
    let observed =
        unsafe { observer.get::<unsafe extern "C" fn() -> u64>(b"luabuddy_observed_calls_v1\0")? };
    // SAFETY: The diagnostic takes no pointers and reads an atomic counter.
    if unsafe { observed() } != 1 {
        return Err(
            "Rust string extension did not observe exactly one live game method call".into(),
        );
    }
    let result = env
        .call_static_method(
            "java/lang/Integer",
            "sum",
            "(II)I",
            &[JValue::Int(2), JValue::Int(3)],
        )?
        .i()?;
    if result != 10 {
        return Err(
            format!("JVM argument mutation did not affect the original method: {result}").into(),
        );
    }
    if *argument_order
        .lock()
        .map_err(|_| "Proof observation lock poisoned")?
        != [2, 7]
    {
        return Err(
            "Co-located native patch priority did not preserve before/mutate/after argument order"
                .into(),
        );
    }
    patcher.disable(env, "proof/sum")?;
    let result = env
        .call_static_method(
            "java/lang/Integer",
            "sum",
            "(II)I",
            &[JValue::Int(2), JValue::Int(3)],
        )?
        .i()?;
    if result != 5
        || *argument_order
            .lock()
            .map_err(|_| "Proof observation lock poisoned")?
            != [2, 7, 2, 2]
    {
        return Err(
            "Disabling one shared receipt removed or retained the wrong native hook".into(),
        );
    }
    patcher.disable(env, "proof/sum-before")?;
    let result = env
        .call_static_method(
            "java/lang/Integer",
            "sum",
            "(II)I",
            &[JValue::Int(2), JValue::Int(3)],
        )?
        .i()?;
    if result != 5
        || *argument_order
            .lock()
            .map_err(|_| "Proof observation lock poisoned")?
            != [2, 7, 2, 2, 2]
    {
        return Err("The remaining shared breakpoint observer did not run exactly once".into());
    }
    let result = env
        .call_static_method(
            "java/lang/Integer",
            "compare",
            "(II)I",
            &[JValue::Int(13), JValue::Int(21)],
        )?
        .i()?;
    if result != -1 || calls.load(Ordering::SeqCst) != 1 {
        return Err("Nested same-target SkipSelf semantics failed".into());
    }
    let threads: Vec<_> = (0..4)
        .map(|i| {
            let vm = Arc::clone(&vm);
            std::thread::spawn(move || -> std::result::Result<i32, String> {
                let mut env = vm.attach_current_thread().map_err(|e| e.to_string())?;
                env.call_static_method(
                    "java/lang/Integer",
                    "compare",
                    "(II)I",
                    &[JValue::Int(i), JValue::Int(i + 1)],
                )
                .and_then(|v| v.i())
                .map_err(|e| e.to_string())
            })
        })
        .collect();
    for thread in threads {
        if thread.join().map_err(|_| "Concurrent caller panicked")?? != -1 {
            return Err("Concurrent caller saw an altered native comparison".into());
        }
    }
    if calls.load(Ordering::SeqCst) != 5 {
        return Err("Concurrent JVM callbacks lost or duplicated an observation".into());
    }

    let mut nested_inner = JvmPatcher::default();
    let mut nested_outer = JvmPatcher::default();
    let inner_target = JvmPatcher::target(env, "java/lang/Integer", "min", "(II)I", "bootstrap")?;
    let outer_target = JvmPatcher::target(env, "java/lang/Integer", "max", "(II)I", "bootstrap")?;
    let inner_calls = Arc::new(AtomicU64::new(0));
    let observed_inner = Arc::clone(&inner_calls);
    nested_inner.register(
        PatchSpec {
            id: "shared".into(),
            owner: "inner".into(),
            target: inner_target,
            kind: HookKind::Before,
            requires: vec![],
            priority: 0,
            reentrancy: Reentrancy::SkipSelf,
        },
        Arc::new(move |_: &mut Invocation| {
            observed_inner.fetch_add(1, Ordering::SeqCst);
            Ok(HookControl::Continue)
        }),
    )?;
    let nested_vm = Arc::clone(&vm);
    nested_outer.register(
        PatchSpec {
            id: "shared".into(),
            owner: "outer".into(),
            target: outer_target,
            kind: HookKind::Before,
            requires: vec![],
            priority: 0,
            reentrancy: Reentrancy::SkipSelf,
        },
        Arc::new(move |_: &mut Invocation| {
            let mut env = nested_vm.get_env().map_err(jni_error)?;
            if env
                .call_static_method(
                    "java/lang/Integer",
                    "min",
                    "(II)I",
                    &[JValue::Int(3), JValue::Int(8)],
                )
                .and_then(|v| v.i())
                .map_err(jni_error)?
                != 3
            {
                return Err(Error::Hook(
                    "Independent nested method result changed".into(),
                ));
            }
            Ok(HookControl::Continue)
        }),
    )?;
    nested_inner.activate(env, &["shared".into()])?;
    nested_outer.activate(env, &["shared".into()])?;
    if env
        .call_static_method(
            "java/lang/Integer",
            "max",
            "(II)I",
            &[JValue::Int(3), JValue::Int(8)],
        )?
        .i()?
        != 8
        || inner_calls.load(Ordering::SeqCst) != 1
    {
        return Err(
            "Independent native registries incorrectly shared a local patch reentrancy name".into(),
        );
    }
    nested_outer.disable(env, "shared")?;
    nested_inner.disable(env, "shared")?;

    let mut exceptional = JvmPatcher::default();
    let parse = JvmPatcher::target(
        env,
        "java/lang/Integer",
        "parseInt",
        "(Ljava/lang/String;)I",
        "bootstrap",
    )?;
    let exception_calls = Arc::new(AtomicU64::new(0));
    let exception_observer = Arc::clone(&exception_calls);
    exceptional.register(
        PatchSpec {
            id: "proof/exception".into(),
            owner: "proof".into(),
            target: parse,
            kind: HookKind::Before,
            requires: vec![],
            priority: 0,
            reentrancy: Reentrancy::SkipSelf,
        },
        Arc::new(move |_: &mut Invocation| {
            exception_observer.fetch_add(1, Ordering::SeqCst);
            Ok(HookControl::Continue)
        }),
    )?;
    exceptional.activate(env, &["proof/exception".into()])?;
    let invalid_number = env.new_string("not-a-number")?;
    let failed = env.call_static_method(
        "java/lang/Integer",
        "parseInt",
        "(Ljava/lang/String;)I",
        &[JValue::Object(&invalid_number)],
    );
    if !matches!(failed, Err(jni::errors::Error::JavaException)) {
        return Err("The original host exception was swallowed by a before hook".into());
    }
    let cause = env.exception_occurred()?;
    env.exception_clear()?;
    if !env.is_instance_of(cause, "java/lang/NumberFormatException")?
        || exception_calls.load(Ordering::SeqCst) != 1
    {
        return Err(
            "The hook changed the original host exception identity or invocation count".into(),
        );
    }
    exceptional.disable(env, "proof/exception")?;

    let mut arrays = JvmPatcher::default();
    let baseline_array = env.new_int_array(1)?;
    if !env
        .call_static_method(
            "java/util/Arrays",
            "equals",
            "([I[I)Z",
            &[
                JValue::Object(baseline_array.as_ref()),
                JValue::Object(baseline_array.as_ref()),
            ],
        )?
        .z()?
    {
        return Err("The JVM array identity baseline is inconsistent".into());
    }
    let equals = JvmPatcher::target(env, "java/util/Arrays", "equals", "([I[I)Z", "bootstrap")?;
    let tokens = Arc::new(Mutex::new(Vec::new()));
    let token_observer = Arc::clone(&tokens);
    arrays.register(
        PatchSpec {
            id: "proof/array-identity".into(),
            owner: "proof".into(),
            target: equals,
            kind: HookKind::Before,
            requires: vec![],
            priority: 0,
            reentrancy: Reentrancy::SkipSelf,
        },
        Arc::new(move |invocation: &mut Invocation| {
            token_observer
                .lock()
                .map_err(|_| Error::Hook("Proof token observation lock poisoned".into()))?
                .push(invocation.arguments.clone());
            Ok(HookControl::Continue)
        }),
    )?;
    arrays.activate(env, &["proof/array-identity".into()])?;
    let second_array = env.new_int_array(2)?;
    let first_array = env.new_int_array(2)?;
    for second in [first_array.as_ref(), second_array.as_ref()] {
        if !env
            .call_static_method(
                "java/util/Arrays",
                "equals",
                "([I[I)Z",
                &[JValue::Object(first_array.as_ref()), JValue::Object(second)],
            )?
            .z()?
        {
            return Err("A reference observer changed the original array equality result".into());
        }
    }
    arrays.disable(env, "proof/array-identity")?;
    if *tokens
        .lock()
        .map_err(|_| "Proof token observation lock poisoned")?
        != [
            vec![Value::Object(1), Value::Object(1)],
            vec![Value::Object(1), Value::Object(2)],
        ]
    {
        return Err(
            "Transient reference tokens did not preserve exact per-invocation object identity"
                .into(),
        );
    }

    let entered = Arc::new(std::sync::Barrier::new(2));
    let released = Arc::new(std::sync::Barrier::new(2));
    let signum = JvmPatcher::target(env, "java/lang/Integer", "signum", "(I)I", "bootstrap")?;
    let mut retiring = JvmPatcher::default();
    let callback_entered = Arc::clone(&entered);
    let callback_released = Arc::clone(&released);
    retiring.register(
        PatchSpec {
            id: "proof/retiring".into(),
            owner: "proof".into(),
            target: signum,
            kind: HookKind::Before,
            requires: vec![],
            priority: 0,
            reentrancy: Reentrancy::SkipSelf,
        },
        Arc::new(move |_: &mut Invocation| {
            callback_entered.wait();
            callback_released.wait();
            Ok(HookControl::Continue)
        }),
    )?;
    retiring.activate(env, &["proof/retiring".into()])?;
    let caller_vm = Arc::clone(&vm);
    let caller = std::thread::spawn(move || -> std::result::Result<i32, String> {
        let mut env = caller_vm
            .attach_current_thread()
            .map_err(|e| e.to_string())?;
        env.call_static_method("java/lang/Integer", "signum", "(I)I", &[JValue::Int(15)])
            .and_then(|v| v.i())
            .map_err(|e| e.to_string())
    });
    entered.wait();
    retiring.disable(env, "proof/retiring")?;
    let subsequent = env
        .call_static_method("java/lang/Integer", "signum", "(I)I", &[JValue::Int(-15)])?
        .i()?;
    released.wait();
    if subsequent != -1
        || caller
            .join()
            .map_err(|_| "Retiring callback caller panicked")??
            != 1
    {
        return Err(
            "Concurrent disable did not preserve an entered call and reject later entries".into(),
        );
    }

    let mut stale = JvmPatcher::default();
    let mut bad_target = math.clone();
    bad_target.bytecode_sha256 = "0".repeat(64);
    stale.register(
        PatchSpec {
            id: "proof/stale".into(),
            owner: "proof".into(),
            target: bad_target,
            kind: HookKind::Before,
            requires: vec![],
            priority: 0,
            reentrancy: Reentrancy::SkipSelf,
        },
        Arc::new(|_: &mut Invocation| Ok(HookControl::Continue)),
    )?;
    if !matches!(
        stale.activate(env, &["proof/stale".into()]),
        Err(Error::StaleTarget(_))
    ) {
        return Err("A stale target fingerprint was accepted".into());
    }
    let mut conflict = JvmPatcher::default();
    conflict.register(
        PatchSpec {
            id: "proof/conflict".into(),
            owner: "proof".into(),
            target: math,
            kind: HookKind::Before,
            requires: vec![],
            priority: 0,
            reentrancy: Reentrancy::SkipSelf,
        },
        Arc::new(|_: &mut Invocation| Ok(HookControl::Continue)),
    )?;
    if !matches!(
        conflict.activate(env, &["proof/conflict".into()]),
        Err(Error::Conflict(_))
    ) {
        return Err("Overlapping native breakpoint ownership was accepted".into());
    }
    let mut unsupported = JvmPatcher::default();
    unsupported.register(
        PatchSpec {
            id: "proof/unsupported".into(),
            owner: "proof".into(),
            target: compare,
            kind: HookKind::AfterNormalReturn,
            requires: vec![],
            priority: 0,
            reentrancy: Reentrancy::SkipSelf,
        },
        Arc::new(|_: &mut Invocation| Ok(HookControl::Continue)),
    )?;
    if !matches!(
        unsupported.activate(env, &["proof/unsupported".into()]),
        Err(Error::Unsupported(_))
    ) {
        return Err("An unimplemented after-return capability was accepted".into());
    }
    for id in [
        "math-guard/live",
        "string-observer/live",
        "proof/compare",
        "proof/sum-after",
    ] {
        patcher.disable(env, id)?;
    }
    let restored = env
        .call_static_method("java/lang/Math", "abs", "(I)I", &[JValue::Int(i32::MIN)])?
        .i()?;
    if restored != i32::MIN {
        return Err(
            "Disabling the math extension did not restore the original JVM behavior".into(),
        );
    }
    let restored = env
        .call_static_method(
            "java/lang/Integer",
            "sum",
            "(II)I",
            &[JValue::Int(2), JValue::Int(3)],
        )?
        .i()?;
    if restored != 5 {
        return Err("Disabling argument mutation did not restore original arguments".into());
    }
    if *argument_order
        .lock()
        .map_err(|_| "Proof observation lock poisoned")?
        != [2, 7, 2, 2, 2]
    {
        return Err("The final shared breakpoint receipt remained active after disable".into());
    }
    let _ = env
        .call_static_method(
            "zombie/util/StringUtils",
            "isNullOrEmpty",
            "(Ljava/lang/String;)Z",
            &[JValue::Object(&empty)],
        )?
        .z()?;
    // SAFETY: The same scalar diagnostic remains mapped throughout this proof.
    if unsafe { observed() } != 1 {
        return Err("A disabled observer still ran".into());
    }
    Ok(())
}
