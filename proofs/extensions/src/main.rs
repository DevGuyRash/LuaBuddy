use jni::{
    InitArgsBuilder, JNIEnv, JNIVersion, JavaVM,
    objects::{JByteArray, JObject, JString, JValue},
};
use luabuddy_patcher::{
    HookKind, PatchSpec, Reentrancy, Target,
    extension::{ExtensionManifest, Privilege},
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::Duration,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run(command: &mut Command) -> Result<Output> {
    let output = command.output()?;
    if !output.status.success() {
        return Err(format!(
            "Command failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(output)
}

fn rawget<'vm>(
    env: &mut JNIEnv<'vm>,
    table: &JObject<'_>,
    name: &str,
) -> jni::errors::Result<JObject<'vm>> {
    let name = env.new_string(name)?;
    env.call_method(
        table,
        "rawget",
        "(Ljava/lang/Object;)Ljava/lang/Object;",
        &[JValue::Object(&name)],
    )?
    .l()
}

fn class_bytes(env: &mut JNIEnv<'_>, class: &str) -> Result<Vec<u8>> {
    let path = env.new_string(format!("{class}.class"))?;
    let stream = env
        .call_static_method(
            "java/lang/ClassLoader",
            "getSystemResourceAsStream",
            "(Ljava/lang/String;)Ljava/io/InputStream;",
            &[JValue::Object(&path)],
        )?
        .l()?;
    if stream.is_null() {
        return Err(format!("Actual JVM class resource missing: {class}").into());
    }
    let bytes = env.call_method(&stream, "readAllBytes", "()[B", &[])?.l()?;
    env.call_method(stream, "close", "()V", &[])?;
    Ok(env.convert_byte_array(JByteArray::from(bytes))?)
}

fn method_code(bytes: &[u8], method: &str, descriptor: &str) -> Result<Vec<u8>> {
    struct Cursor<'a> {
        bytes: &'a [u8],
        at: usize,
    }
    impl<'a> Cursor<'a> {
        fn take(&mut self, size: usize) -> Result<&'a [u8]> {
            let end = self
                .at
                .checked_add(size)
                .ok_or("Classfile offset overflow")?;
            let value = self
                .bytes
                .get(self.at..end)
                .ok_or("Truncated actual classfile")?;
            self.at = end;
            Ok(value)
        }
        fn u16(&mut self) -> Result<usize> {
            Ok(u16::from_be_bytes(self.take(2)?.try_into()?) as usize)
        }
        fn u32(&mut self) -> Result<usize> {
            Ok(u32::from_be_bytes(self.take(4)?.try_into()?) as usize)
        }
    }
    let mut c = Cursor { bytes, at: 0 };
    if c.take(4)? != [0xca, 0xfe, 0xba, 0xbe] {
        return Err("Invalid actual classfile header".into());
    }
    c.take(4)?;
    let count = c.u16()?;
    let mut strings = vec![None; count];
    let mut index = 1;
    while index < count {
        let tag = c.take(1)?[0];
        match tag {
            1 => {
                let size = c.u16()?;
                strings[index] = Some(c.take(size)?.to_vec());
            }
            3 | 4 | 9 | 10 | 11 | 12 | 17 | 18 => {
                c.take(4)?;
            }
            5 | 6 => {
                c.take(8)?;
                index += 1;
            }
            7 | 8 | 16 | 19 | 20 => {
                c.take(2)?;
            }
            15 => {
                c.take(3)?;
            }
            _ => return Err(format!("Unsupported actual classfile constant tag {tag}").into()),
        }
        index += 1;
    }
    c.take(6)?;
    let interfaces = c.u16()?;
    c.take(interfaces * 2)?;
    for group in 0..2 {
        let members = c.u16()?;
        for _ in 0..members {
            c.u16()?;
            let name = c.u16()?;
            let desc = c.u16()?;
            let attributes = c.u16()?;
            for _ in 0..attributes {
                let attribute = c.u16()?;
                let size = c.u32()?;
                let body = c.take(size)?;
                if group == 1
                    && strings.get(name).and_then(Option::as_deref) == Some(method.as_bytes())
                    && strings.get(desc).and_then(Option::as_deref) == Some(descriptor.as_bytes())
                    && strings.get(attribute).and_then(Option::as_deref) == Some(b"Code")
                {
                    let mut code = Cursor { bytes: body, at: 0 };
                    code.take(4)?;
                    let size = code.u32()?;
                    return Ok(code.take(size)?.to_vec());
                }
            }
        }
    }
    Err(format!("Actual method not found: {method}{descriptor}").into())
}

fn manifest(
    env: &mut JNIEnv<'_>,
    id: &str,
    artifact: &Path,
    target: [&str; 4],
    kind: HookKind,
    privilege: Privilege,
) -> Result<ExtensionManifest> {
    let [class, method, descriptor, loader] = target;
    let code = method_code(&class_bytes(env, class)?, method, descriptor)?;
    Ok(ExtensionManifest {
        schema_version: 1,
        abi_version: 1,
        id: id.into(),
        artifact_sha256: format!("{:x}", Sha256::digest(fs::read(artifact)?)),
        provenance: "reviewed repository Rust product proof".into(),
        target_os: std::env::consts::OS.into(),
        target_arch: std::env::consts::ARCH.into(),
        privileges: BTreeSet::from([privilege]),
        patches: vec![PatchSpec {
            id: format!("{id}/product"),
            owner: id.into(),
            target: Target {
                class: class.into(),
                method: method.into(),
                descriptor: descriptor.into(),
                loader: loader.into(),
                bytecode_sha256: format!("{:x}", Sha256::digest(code)),
            },
            kind,
            requires: vec![],
            priority: 0,
            reentrancy: Reentrancy::SkipSelf,
        }],
    })
}

fn approve(
    exe: &Path,
    fixture: &Path,
    artifact: &Path,
    manifest: &ExtensionManifest,
    enabled: bool,
) -> Result<()> {
    let file = fixture.join(format!("{}.json", manifest.id));
    fs::write(&file, serde_json::to_vec_pretty(manifest)?)?;
    let manifest_digest = manifest_digest(manifest)?;
    let privileges = manifest
        .privileges
        .iter()
        .map(|value| serde_json::to_value(value).map(|v| v.as_str().unwrap_or_default().to_owned()))
        .collect::<std::result::Result<Vec<_>, _>>()?
        .join(",");
    let profiles = fixture.join("config");
    let mut install = Command::new(exe);
    install
        .args(["extension-install", "--profiles-dir"])
        .arg(&profiles)
        .arg("--artifact")
        .arg(artifact)
        .arg("--manifest")
        .arg(&file)
        .args([
            "--sha256",
            &manifest.artifact_sha256,
            "--manifest-sha256",
            &manifest_digest,
            "--provenance",
            &manifest.provenance,
            "--allow",
            &privileges,
            "--trust-native-code",
        ]);
    run(&mut install)?;
    if enabled {
        let mut enable = Command::new(exe);
        enable
            .args(["extension-enable", "--profiles-dir"])
            .arg(&profiles)
            .args([
                "--extension-id",
                &manifest.id,
                "--sha256",
                &manifest.artifact_sha256,
                "--manifest-sha256",
                &manifest_digest,
                "--provenance",
                &manifest.provenance,
                "--allow",
                &privileges,
                "--trust-native-code",
            ]);
        run(&mut enable)?;
    }
    Ok(())
}

fn manifest_digest(manifest: &ExtensionManifest) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec_pretty(manifest)?)
    ))
}

fn generation(fixture: &Path, manifest: &ExtensionManifest) -> Result<PathBuf> {
    Ok(fixture
        .join("config/extensions")
        .join(&manifest.id)
        .join(manifest_digest(manifest)?))
}

fn initialize_host(env: &mut JNIEnv<'_>, fixture: &Path) -> Result<()> {
    for name in [
        "zombie/core/random/RandStandard",
        "zombie/core/random/RandLua",
    ] {
        let instance = env
            .get_static_field(name, "INSTANCE", format!("L{name};"))?
            .l()?;
        env.call_method(instance, "init", "()V", &[])?;
    }
    let filesystem = env
        .get_static_field(
            "zombie/ZomboidFileSystem",
            "instance",
            "Lzombie/ZomboidFileSystem;",
        )?
        .l()?;
    let cache = env.new_string(fixture.join("cache").to_string_lossy())?;
    env.call_method(
        &filesystem,
        "setCacheDir",
        "(Ljava/lang/String;)V",
        &[JValue::Object(&cache)],
    )?;
    env.call_method(filesystem, "init", "()V", &[])?;
    let languages = env
        .get_static_field(
            "zombie/core/Languages",
            "instance",
            "Lzombie/core/Languages;",
        )?
        .l()?;
    env.call_method(languages, "init", "()V", &[])?;
    env.call_static_method("zombie/Lua/LuaManager", "init", "()V", &[])?;
    let client = env.get_static_field_id("zombie/network/GameClient", "client", "Z")?;
    env.set_static_field("zombie/network/GameClient", client, JValue::Bool(1))?;
    Ok(())
}

fn startup<'vm>(env: &mut JNIEnv<'vm>) -> Result<JObject<'vm>> {
    let environment = env
        .get_static_field(
            "zombie/Lua/LuaManager",
            "env",
            "Lse/krka/kahlua/vm/KahluaTable;",
        )?
        .l()?;
    let math = rawget(env, &environment, "math")?;
    let callback = rawget(env, &math, "max")?;
    if !env.is_instance_of(&callback, "se/krka/kahlua/vm/LuaClosure")? {
        return Err("Expected the existing host stdlib LuaClosure".into());
    }
    let event_name = env.new_string("OnGameStart")?;
    let event = env
        .call_static_method(
            "zombie/Lua/LuaEventManager",
            "AddEvent",
            "(Ljava/lang/String;)Lzombie/Lua/Event;",
            &[JValue::Object(&event_name)],
        )?
        .l()?;
    let callbacks = env
        .get_field(event, "callbacks", "Ljava/util/ArrayList;")?
        .l()?;
    env.call_method(
        callbacks,
        "add",
        "(Ljava/lang/Object;)Z",
        &[JValue::Object(&callback)],
    )?;
    let one = env
        .call_static_method(
            "java/lang/Double",
            "valueOf",
            "(D)Ljava/lang/Double;",
            &[JValue::Double(1.0)],
        )?
        .l()?;
    env.call_static_method(
        "zombie/Lua/LuaEventManager",
        "triggerEvent",
        "(Ljava/lang/String;Ljava/lang/Object;Ljava/lang/Object;)V",
        &[
            JValue::Object(&event_name),
            JValue::Object(&one),
            JValue::Object(&one),
        ],
    )?;
    let state = rawget(env, &environment, "LuaBuddy")?;
    if state.is_null() {
        return Err("Normal production agent did not create LuaBuddy".into());
    }
    Ok(state)
}

fn retry(env: &mut JNIEnv<'_>, state: &JObject<'_>) -> Result<()> {
    let function = rawget(env, state, "retryPatches")?;
    if function.is_null() {
        return Err("Normal product has no explicit approved-patch retry callable".into());
    }
    let thread = env
        .get_static_field(
            "zombie/Lua/LuaManager",
            "thread",
            "Lse/krka/kahlua/vm/KahluaThread;",
        )?
        .l()?;
    let args = env.new_object_array(0, "java/lang/Object", JObject::null())?;
    let result = env
        .call_method(
            thread,
            "call",
            "(Ljava/lang/Object;[Ljava/lang/Object;)Ljava/lang/Object;",
            &[JValue::Object(&function), JValue::Object(&args)],
        )?
        .l()?;
    if !env.call_method(result, "booleanValue", "()Z", &[])?.z()? {
        return Err("Explicit patch retry was refused".into());
    }
    Ok(())
}

fn scenario(game: &Path, artifacts: &Path, mode: &str) -> Result<()> {
    if !matches!(
        mode,
        "disabled"
            | "unapproved"
            | "mismatch"
            | "manifest-tamper"
            | "metadata-update"
            | "active"
            | "deferred"
    ) {
        return Err("Unknown product extension verification mode".into());
    }
    let path = artifacts.join(format!("case-{mode}"));
    fs::create_dir(&path)?;
    let fixture = Fixture(path);
    fs::create_dir(fixture.0.join("config"))?;
    fs::create_dir(fixture.0.join("home"))?;
    let marker = fixture.0.join("native-initializer.txt");
    // SAFETY: This standalone process has not created JVM or worker threads yet.
    unsafe {
        std::env::set_var("LUABUDDY_EXTENSION_PROOF_MARKER", &marker);
    }
    let agent = fixture.0.join("runtime.so");
    fs::copy(artifacts.join("runtime.so"), &agent)?;
    println!(
        "Normal product runtime SHA256 {:x}",
        Sha256::digest(fs::read(&agent)?)
    );
    let options = InitArgsBuilder::new()
        .version(JNIVersion::V8)
        .option(format!(
            "-Djava.class.path={}",
            game.join("projectzomboid.jar").display()
        ))
        .option(format!(
            "-Djava.library.path={}",
            game.join("natives").display()
        ))
        .option(format!("-Duser.home={}", fixture.0.join("home").display()))
        .option(format!(
            "-Dluabuddy.config={}",
            fixture.0.join("config").display()
        ))
        .option(format!("-agentpath:{}", agent.display()))
        .option("-Dzomboid.steam=0")
        .option("-Xcheck:jni")
        .option("-Xverify:all")
        .option("--enable-native-access=ALL-UNNAMED")
        .build()?;
    std::env::set_current_dir(game)?;
    let vm = JavaVM::new(options)?;
    let mut env = vm.attach_current_thread()?;
    let result = (|| -> Result<()> {
        initialize_host(&mut env, &fixture.0)?;
        let minimum = env
            .call_static_method("java/lang/Math", "abs", "(I)I", &[JValue::Int(i32::MIN)])?
            .i()?;
        if minimum != i32::MIN {
            return Err("The unpatched JVM baseline is wrong".into());
        }
        let empty = env.new_string("")?;
        if !env
            .call_static_method(
                "zombie/util/StringUtils",
                "isNullOrEmpty",
                "(Ljava/lang/String;)Z",
                &[JValue::Object(&empty)],
            )?
            .z()?
        {
            return Err("Host StringUtils baseline failed".into());
        }
        let exe = artifacts.join("luabuddy");
        let math_path = artifacts.join("math.so");
        let observer_path = artifacts.join("observer.so");
        let marker_path = artifacts.join("marker.so");
        let math_manifest = manifest(
            &mut env,
            "math-guard",
            &math_path,
            ["java/lang/Math", "abs", "(I)I", "bootstrap"],
            HookKind::Replace,
            Privilege::ReplaceMethods,
        )?;
        let observer_manifest = manifest(
            &mut env,
            "string-observer",
            &observer_path,
            [
                "zombie/util/StringUtils",
                "isNullOrEmpty",
                "(Ljava/lang/String;)Z",
                "system",
            ],
            HookKind::Before,
            Privilege::ObserveMethods,
        )?;
        let mut marker_manifest = manifest(
            &mut env,
            "marker-extension",
            &marker_path,
            if mode == "deferred" {
                ["zombie/util/FrameDelay", "update", "()Z", "system"]
            } else {
                ["java/lang/Math", "abs", "(I)I", "bootstrap"]
            },
            HookKind::Before,
            Privilege::ObserveMethods,
        )?;
        if mode == "unapproved" {
            let unapproved = fixture.0.join("config/extensions/marker-extension");
            fs::create_dir_all(&unapproved)?;
            fs::copy(&marker_path, unapproved.join("artifact.so"))?;
        } else {
            let enabled = matches!(
                mode,
                "active" | "mismatch" | "deferred" | "metadata-update" | "manifest-tamper"
            );
            approve(&exe, &fixture.0, &math_path, &math_manifest, enabled)?;
            approve(
                &exe,
                &fixture.0,
                &observer_path,
                &observer_manifest,
                enabled,
            )?;
            approve(&exe, &fixture.0, &marker_path, &marker_manifest, enabled)?;
            if mode == "mismatch" {
                let path = generation(&fixture.0, &marker_manifest)?.join("artifact.so");
                let mut bytes = fs::read(&path)?;
                bytes.extend_from_slice(b"unapproved edit");
                fs::write(path, bytes)?;
            }
            if mode == "manifest-tamper" {
                let path = generation(&fixture.0, &marker_manifest)?.join("manifest.json");
                let mut edited = marker_manifest.clone();
                edited.patches[0].priority = 19;
                fs::write(path, serde_json::to_vec_pretty(&edited)?)?;
            }
            if mode == "metadata-update" {
                let old_generation = generation(&fixture.0, &marker_manifest)?;
                let old_manifest = fs::read(old_generation.join("manifest.json"))?;
                let old_artifact = fs::read(old_generation.join("artifact.so"))?;
                let old_digest = manifest_digest(&marker_manifest)?;
                let mut replacement = marker_manifest.clone();
                replacement.patches[0].target = observer_manifest.patches[0].target.clone();
                replacement.patches[0].priority = 17;
                approve(&exe, &fixture.0, &marker_path, &replacement, false)?;
                let approval = fixture
                    .0
                    .join("config/extensions/marker-extension/approval.json");
                let saved = fs::read(&approval)?;
                let output = Command::new(&exe)
                    .args(["extension-enable", "--profiles-dir"])
                    .arg(fixture.0.join("config"))
                    .args([
                        "--extension-id",
                        "marker-extension",
                        "--sha256",
                        &marker_manifest.artifact_sha256,
                        "--manifest-sha256",
                        &old_digest,
                        "--provenance",
                        &marker_manifest.provenance,
                        "--allow",
                        "observe_methods",
                        "--trust-native-code",
                    ])
                    .output()?;
                if output.status.success() || fs::read(&approval)? != saved {
                    return Err("A changed manifest reused an old native approval".into());
                }
                if fs::read(old_generation.join("manifest.json"))? != old_manifest
                    || fs::read(old_generation.join("artifact.so"))? != old_artifact
                {
                    return Err("Metadata update modified the prior immutable generation".into());
                }
                marker_manifest = replacement;
            }
        }
        let state = startup(&mut env)?;
        let patches = rawget(&mut env, &state, "patches")?;
        if patches.is_null() {
            return Err("Normal product did not publish patch diagnostics".into());
        }
        let hooks = rawget(&mut env, &patches, "runtimeHooks")?;
        if env.call_method(&hooks, "size", "()I", &[])?.i()? != 3 {
            return Err("Actual adapter hook inventory is incomplete".into());
        }
        for index in 1..=3 {
            let hook = env
                .call_method(
                    &hooks,
                    "rawget",
                    "(I)Ljava/lang/Object;",
                    &[JValue::Int(index)],
                )?
                .l()?;
            let status = JString::from(rawget(&mut env, &hook, "state")?);
            if env.get_string(&status)?.to_string_lossy() != "installed" {
                return Err(
                    "A built-in hook was reported inactive on the tested compatible host".into(),
                );
            }
        }
        let actual = env
            .call_static_method("java/lang/Math", "abs", "(I)I", &[JValue::Int(i32::MIN)])?
            .i()?;
        let entries = rawget(&mut env, &patches, "extensions")?;
        let mut installed = 0;
        let mut rejected = 0;
        let count = env.call_method(&entries, "size", "()I", &[])?.i()?;
        for index in 1..=count {
            let entry = env
                .call_method(
                    &entries,
                    "rawget",
                    "(I)Ljava/lang/Object;",
                    &[JValue::Int(index)],
                )?
                .l()?;
            let value = rawget(&mut env, &entry, "state")?;
            let text = JString::from(value);
            let text: String = env.get_string(&text)?.into();
            if text == "installed" {
                installed += 1;
            }
            if text == "rejected" {
                rejected += 1;
            }
        }
        match mode {
            "active" => {
                if actual != i32::MAX || installed != 3 || !marker.is_file() {
                    return Err(format!("Approved product extensions did not all activate: abs={actual} installed={installed} marker={}",marker.exists()).into());
                }
                let observer = generation(&fixture.0, &observer_manifest)?.join("artifact.so");
                // SAFETY: The reviewed example is already mapped by the normal product and exports a scalar diagnostic.
                let library = unsafe { libloading::Library::new(observer)? };
                let calls = unsafe {
                    library.get::<unsafe extern "C" fn() -> u64>(b"luabuddy_observed_calls_v1\0")?
                };
                let before = unsafe { calls() };
                env.call_static_method(
                    "zombie/util/StringUtils",
                    "isNullOrEmpty",
                    "(Ljava/lang/String;)Z",
                    &[JValue::Object(&empty)],
                )?;
                let after = unsafe { calls() };
                if after != before + 1 {
                    return Err(
                        "Independent compiled observer did not see the actual host call".into(),
                    );
                }
            }
            "disabled" => {
                if actual != i32::MIN || installed != 0 || marker.exists() {
                    return Err("Installed-disabled extensions executed native code".into());
                }
                approve(&exe, &fixture.0, &math_path, &math_manifest, true)?;
                approve(&exe, &fixture.0, &observer_path, &observer_manifest, true)?;
                approve(&exe, &fixture.0, &marker_path, &marker_manifest, true)?;
                retry(&mut env, &state)?;
                let unchanged = env
                    .call_static_method("java/lang/Math", "abs", "(I)I", &[JValue::Int(i32::MIN)])?
                    .i()?;
                if unchanged != i32::MIN || marker.exists() {
                    return Err("Changed approvals were silently activated in a running JVM".into());
                }
            }
            "unapproved" => {
                if actual != i32::MIN || installed != 0 || marker.exists() || rejected != 1 {
                    return Err("Unapproved native code was executed or not diagnosed".into());
                }
            }
            "mismatch" | "manifest-tamper" => {
                if marker.exists() || rejected < 1 {
                    return Err(
                        "Digest-mismatched native initializer executed or lacked diagnosis".into(),
                    );
                }
            }
            "metadata-update" => {
                if marker.exists() || installed != 2 || actual != i32::MAX {
                    return Err("Same-binary metadata update reused execution consent or disrupted healthy extensions".into());
                }
                let approval: serde_json::Value = serde_json::from_slice(&fs::read(
                    fixture
                        .0
                        .join("config/extensions/marker-extension/approval.json"),
                )?)?;
                if approval["manifest_sha256"] != manifest_digest(&marker_manifest)?
                    || approval["consent"]["enable"] != false
                {
                    return Err(
                        "Selected metadata generation does not have a distinct disabled approval"
                            .into(),
                    );
                }
            }
            "deferred" => {
                if marker.exists() || rejected < 1 {
                    return Err("Unprepared target loaded native initializer before compatibility preflight".into());
                }
                let delay = env.new_object("zombie/util/FrameDelay", "(I)V", &[JValue::Int(2)])?;
                env.call_method(&delay, "update", "()Z", &[])?;
                if marker.exists() {
                    return Err("Ordinary class preparation silently loaded native code without explicit retry".into());
                }
                retry(&mut env, &state)?;
                if !marker.is_file() {
                    return Err(
                        "Explicit retry did not activate the original approved pending generation"
                            .into(),
                    );
                }
                let patches = rawget(&mut env, &state, "patches")?;
                let extensions = rawget(&mut env, &patches, "extensions")?;
                let mut found = false;
                for index in 1..=env.call_method(&extensions, "size", "()I", &[])?.i()? {
                    let entry = env
                        .call_method(
                            &extensions,
                            "rawget",
                            "(I)Ljava/lang/Object;",
                            &[JValue::Int(index)],
                        )?
                        .l()?;
                    let id = JString::from(rawget(&mut env, &entry, "id")?);
                    if env.get_string(&id)?.to_string_lossy() == "marker-extension" {
                        let state = JString::from(rawget(&mut env, &entry, "state")?);
                        found = env.get_string(&state)?.to_string_lossy() == "installed";
                    }
                }
                if !found {
                    return Err("Retried extension lacks an actual installed receipt".into());
                }
            }
            _ => return Err("Unsupported proof mode".into()),
        }
        println!(
            "PASS normal product extension {mode}: initialInstalled={installed} initialRejected={rejected}, native initializer checked"
        );
        Ok(())
    })();
    if result.is_err() && env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }
    result
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() == 3 {
        return scenario(
            &PathBuf::from(&args[0]),
            &PathBuf::from(&args[1]),
            &args[2].to_string_lossy(),
        );
    }
    if args.len() != 2 {
        return Err("Provide actual game directory and repository root".into());
    }
    let game = PathBuf::from(&args[0]).canonicalize()?;
    let repo = PathBuf::from(&args[1]).canonicalize()?;
    let exe = std::env::current_exe()?;
    let path =
        std::env::temp_dir().join(format!("luabuddy-extension-matrix-{}", std::process::id()));
    fs::create_dir(&path)?;
    let artifacts = Fixture(path);
    let runtime = std::env::var_os("LUABUDDY_PROOF_RUNTIME")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo.join("target/debug/libluabuddy_host.so"));
    let cli = std::env::var_os("LUABUDDY_PROOF_CLI")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo.join("target/debug/luabuddy"));
    fs::copy(runtime, artifacts.0.join("runtime.so"))?;
    fs::copy(cli, artifacts.0.join("luabuddy"))?;
    println!(
        "Normal product CLI SHA256 {:x}",
        Sha256::digest(fs::read(artifacts.0.join("luabuddy"))?)
    );
    for (source, name) in [
        (
            "target/extensions/debug/libluabuddy_math_guard.so",
            "math.so",
        ),
        (
            "target/extensions-observer/debug/libluabuddy_string_observer.so",
            "observer.so",
        ),
        (
            "target/extensions-marker/debug/libluabuddy_extension_marker_proof.so",
            "marker.so",
        ),
    ] {
        fs::copy(repo.join(source), artifacts.0.join(name))?;
    }
    for mode in [
        "disabled",
        "unapproved",
        "mismatch",
        "manifest-tamper",
        "metadata-update",
        "active",
        "deferred",
    ] {
        let mut child = Command::new(&exe)
            .arg(&game)
            .arg(&artifacts.0)
            .arg(mode)
            .spawn()?;
        let started = std::time::Instant::now();
        loop {
            if let Some(status) = child.try_wait()? {
                if !status.success() {
                    return Err(format!("Normal product extension {mode} failed").into());
                }
                break;
            }
            if started.elapsed() > Duration::from_secs(45) {
                child.kill()?;
                child.wait()?;
                return Err(format!("Normal product extension {mode} timed out").into());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    println!(
        "PASS normal production-agent extension delivery matrix; no account or user-game changes"
    );
    Ok(())
}
