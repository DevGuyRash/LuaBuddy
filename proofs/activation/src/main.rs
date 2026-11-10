#![allow(dead_code)]
#[path = "../../../crates/luabuddy-host/src/activation.rs"]
mod activation;
#[path = "../../../crates/luabuddy-host/src/game.rs"]
mod game;
#[path = "../../../crates/luabuddy-host/src/lua.rs"]
mod lua;

use jni::{InitArgsBuilder, JNIVersion, JavaVM, objects::JValue};
use luabuddy_core::{ModId, Profile};
use std::path::{Path, PathBuf};

struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write(root: &Path, name: &str, text: &str) -> std::io::Result<()> {
    let path = root.join(name);
    std::fs::create_dir_all(
        path.parent()
            .ok_or_else(|| std::io::Error::other("Fixture file needs a parent"))?,
    )?;
    std::fs::write(path, text)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let game = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("Provide the installed game directory")?
        .canonicalize()?;
    let fixture_path = std::env::temp_dir().join(format!(
        "luabuddy-activation-proof-{} space-雪",
        std::process::id()
    ));
    std::fs::create_dir(&fixture_path)?;
    let fixture = Fixture(fixture_path);
    std::fs::create_dir(fixture.0.join("home"))?;
    std::fs::create_dir(fixture.0.join("cache"))?;
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
        .option("-Dzomboid.steam=0")
        .option("-Xcheck:jni")
        .option("-Xverify:all")
        .option("--enable-native-access=ALL-UNNAMED")
        .build()?;
    std::env::set_current_dir(&game)?;
    let vm = JavaVM::new(options)?;
    let mut env = vm.attach_current_thread()?;
    let result = run(&mut env, &fixture.0);
    if let Err(error) = result {
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_describe();
            let _ = env.exception_clear();
        }
        return Err(error.into());
    }
    println!("PASS native startup selection and centralized live eligibility");
    Ok(())
}

fn run(env: &mut jni::JNIEnv<'_>, root: &Path) -> lua::Result<()> {
    for class in [
        "zombie/core/random/RandStandard",
        "zombie/core/random/RandLua",
    ] {
        let instance = env
            .get_static_field(class, "INSTANCE", format!("L{class};"))
            .and_then(|value| value.l())
            .map_err(|error| error.to_string())?;
        env.call_method(instance, "init", "()V", &[])
            .map_err(|error| error.to_string())?;
    }
    let filesystem = env
        .get_static_field(
            "zombie/ZomboidFileSystem",
            "instance",
            "Lzombie/ZomboidFileSystem;",
        )
        .and_then(|value| value.l())
        .map_err(|error| error.to_string())?;
    let cache = env
        .new_string(root.join("cache").to_string_lossy())
        .map_err(|error| error.to_string())?;
    env.call_method(
        &filesystem,
        "setCacheDir",
        "(Ljava/lang/String;)V",
        &[JValue::Object(&cache)],
    )
    .map_err(|error| error.to_string())?;
    env.call_method(&filesystem, "init", "()V", &[])
        .map_err(|error| error.to_string())?;
    env.call_static_method("zombie/Lua/LuaManager", "init", "()V", &[])
        .map_err(|error| error.to_string())?;
    let mut metadata = [
        ("NativeLibrary", ""),
        ("NativeConsumer", "require=NativeLibrary\n"),
        ("Native 雪", ""),
        ("NativeMissing", "require=NotInstalled\n"),
        ("NativeFuture", "versionMin=999.0\n"),
        ("NativeCycleA", "require=NativeCycleB\n"),
        ("NativeCycleB", "require=NativeCycleA\n"),
    ]
    .into_iter()
    .map(|(id, extra)| (id.to_owned(), extra.to_owned()))
    .collect::<Vec<_>>();
    for index in 0..128 {
        metadata.push((
            format!("NativeChain{index}"),
            if index < 127 {
                format!("require=NativeChain{}\n", index + 1)
            } else {
                String::new()
            },
        ));
    }
    for (id, extra) in metadata {
        write(
            root,
            &format!("mods/{id}/42/mod.info"),
            &format!("id={id}\nname={id}\n{extra}"),
        )
        .map_err(|error| error.to_string())?;
        env.with_local_frame(8, |env| -> jni::errors::Result<()> {
            let id_object = env.new_string(&id)?;
            let directory = env.new_string(root.join("mods").join(&id).to_string_lossy())?;
            env.call_method(
                &filesystem,
                "setModIdToDir",
                "(Ljava/lang/String;Ljava/lang/String;)V",
                &[JValue::Object(&id_object), JValue::Object(&directory)],
            )?;
            Ok(())
        })
        .map_err(|error| error.to_string())?;
    }
    let mut context = lua::Context::current(env)?;
    let version = context.jni(|env| {
        let core = env
            .call_static_method(
                "zombie/core/Core",
                "getInstance",
                "()Lzombie/core/Core;",
                &[],
            )?
            .l()?;
        let value = env
            .call_method(core, "getGameAndBuildVersion", "()Ljava/lang/String;", &[])?
            .l()?;
        env.new_global_ref(value)
            .map(|value| lua::Value(Some(value)))
    })?;
    println!(
        "Actual game {} with Java {}",
        context.text(&version)?,
        game::property(&mut context, "java.version")?.unwrap_or_default()
    );
    let default = named(&mut context, "default")?;
    let current = named(&mut context, "currentGame")?;
    append(&mut context, &default, "getMods", "Unrelated")?;
    append(&mut context, &default, "getMapOrder", "UserMapFirst")?;
    append(&mut context, &default, "getMapOrder", "UserMapSecond")?;
    append(&mut context, &current, "getMods", "CurrentWorldOnly")?;
    context.jni(|env| {
        env.call_static_method(
            "zombie/modding/ActiveMods",
            "setLoadedMods",
            "(Lzombie/modding/ActiveMods;)V",
            &[JValue::Object(current.object())],
        )?;
        Ok(())
    })?;
    let active = context.jni(|env| {
        let value = env
            .call_method(&filesystem, "getModIDs", "()Ljava/util/List;", &[])?
            .l()?;
        env.new_global_ref(value)
            .map(|value| lua::Value(Some(value)))
    })?;
    context.jni(|env| {
        let id = env.new_string("ServerApproved")?;
        env.call_method(
            active.object(),
            "add",
            "(Ljava/lang/Object;)Z",
            &[JValue::Object(&id)],
        )?;
        Ok(())
    })?;
    let destination = root.join("cache/mods/default.txt");
    std::fs::create_dir_all(
        destination
            .parent()
            .ok_or("Native preferences need a parent")?,
    )
    .map_err(|error| error.to_string())?;
    if !write_native(&mut context, &destination, &default)? {
        return Err("Fixture native writer failed".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&destination, std::fs::Permissions::from_mode(0o640))
            .map_err(|error| error.to_string())?;
    }
    let consumer = ModId::new("NativeConsumer").map_err(|error| error.to_string())?;
    let library = ModId::new("NativeLibrary").map_err(|error| error.to_string())?;
    let result =
        activation::select_next_startup(&mut context, std::slice::from_ref(&consumer), true)?;
    assert_eq!(result.changed, vec![library.clone(), consumer.clone()]);
    assert_eq!(
        result.selected,
        ["Unrelated", "NativeLibrary", "NativeConsumer"]
            .into_iter()
            .map(|id| ModId::new(id).expect("fixture IDs are valid"))
            .collect()
    );
    assert_native_lists(
        &mut context,
        &destination,
        &["Unrelated", "NativeLibrary", "NativeConsumer"],
        &["UserMapFirst", "UserMapSecond"],
    )?;
    assert_unchanged_live(&mut context, &current, &active)?;
    assert_eq!(
        activation::selected_next_startup(&mut context)?,
        result.selected
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&destination)
                .map_err(|error| error.to_string())?
                .permissions()
                .mode()
                & 0o777,
            0o640
        );
    }
    println!(
        "PASS native dependency selection, durable readback, unrelated mod/map-order and file-mode preservation; live state untouched"
    );
    let before = std::fs::read(&destination).map_err(|error| error.to_string())?;
    let repeated =
        activation::select_next_startup(&mut context, std::slice::from_ref(&consumer), true)?;
    assert!(repeated.changed.is_empty());
    assert_eq!(
        before,
        std::fs::read(&destination).map_err(|error| error.to_string())?
    );
    let refused =
        activation::select_next_startup(&mut context, std::slice::from_ref(&library), false)
            .expect_err("an enabled native dependent must block removal");
    assert!(refused.contains("NativeConsumer"));
    assert_eq!(
        before,
        std::fs::read(&destination).map_err(|error| error.to_string())?
    );
    for id in ["NativeMissing", "NativeCycleA", "NativeFuture"] {
        let id = ModId::new(id).map_err(|error| error.to_string())?;
        assert!(activation::select_next_startup(&mut context, &[id], true).is_err());
        assert_eq!(
            before,
            std::fs::read(&destination).map_err(|error| error.to_string())?
        );
    }
    assert_unchanged_live(&mut context, &current, &active)?;
    println!(
        "PASS repeated no-op, dependent refusal, missing dependency, cycles and incompatible versions without preference mutation"
    );
    activation::select_next_startup(&mut context, &[consumer], false)?;
    activation::select_next_startup(&mut context, &[library], false)?;
    assert_native_lists(
        &mut context,
        &destination,
        &["Unrelated"],
        &["UserMapFirst", "UserMapSecond"],
    )?;
    let unicode = ModId::new("Native 雪").map_err(|error| error.to_string())?;
    activation::select_next_startup(&mut context, std::slice::from_ref(&unicode), true)?;
    assert_native_lists(
        &mut context,
        &destination,
        &["Unrelated", "Native 雪"],
        &["UserMapFirst", "UserMapSecond"],
    )?;
    activation::select_next_startup(&mut context, &[unicode], false)?;
    let chain = ModId::new("NativeChain0").map_err(|error| error.to_string())?;
    activation::select_next_startup(&mut context, &[chain], true)?;
    let expected_chain = std::iter::once("Unrelated".to_owned())
        .chain((0..128).rev().map(|index| format!("NativeChain{index}")))
        .collect::<Vec<_>>();
    let expected_chain = expected_chain
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    assert_native_lists(
        &mut context,
        &destination,
        &expected_chain,
        &["UserMapFirst", "UserMapSecond"],
    )?;
    let chain = (0..128)
        .map(|index| ModId::new(format!("NativeChain{index}")))
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    activation::select_next_startup(&mut context, &chain, false)?;
    assert_native_lists(
        &mut context,
        &destination,
        &["Unrelated"],
        &["UserMapFirst", "UserMapSecond"],
    )?;
    println!("PASS long native dependency chain and bounded JNI reference enumeration");
    assert_unchanged_live(&mut context, &current, &active)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let parent = destination
            .parent()
            .ok_or("Native preference parent missing")?;
        let permissions = std::fs::metadata(parent)
            .map_err(|error| error.to_string())?
            .permissions();
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o500))
            .map_err(|error| error.to_string())?;
        let failure = activation::select_next_startup(
            &mut context,
            &[ModId::new("NativeLibrary").map_err(|error| error.to_string())?],
            true,
        );
        std::fs::set_permissions(parent, permissions).map_err(|error| error.to_string())?;
        assert!(
            failure.is_err(),
            "read-only native preference directory must refuse staging"
        );
        assert_native_lists(
            &mut context,
            &destination,
            &["Unrelated"],
            &["UserMapFirst", "UserMapSecond"],
        )?;
        assert_eq!(
            activation::selected_next_startup(&mut context)?,
            [ModId::new("Unrelated").map_err(|error| error.to_string())?]
                .into_iter()
                .collect()
        );
    }
    assert_unchanged_live(&mut context, &current, &active)?;
    println!("PASS disable and failed native preference write retain unrelated and live state");
    eligibility(root)?;
    Ok(())
}

fn eligibility(root: &Path) -> lua::Result<()> {
    use activation::{Eligibility, StartupComponent};
    let profile = Profile::parse(root.join("selection.properties"), "id=NativeConsumer\n")
        .map_err(|error| error.to_string())?;
    let roots = vec![root.join("managed-content")];
    write(
        &roots[0],
        "media/lua/shared/source.lua",
        "managed content fixture",
    )
    .map_err(|error| error.to_string())?;
    assert_eq!(
        activation::assess(&profile, &roots, &[])?,
        Eligibility::Live {
            limitations: Vec::new()
        }
    );
    write(
        &roots[0],
        "media/scripts/assets.txt",
        "native definitions fixture",
    )
    .map_err(|error| error.to_string())?;
    write(
        &roots[0],
        "media/lua/server/source.lua",
        "managed server content fixture",
    )
    .map_err(|error| error.to_string())?;
    assert!(matches!(
        activation::assess(&profile, &roots, &[])?,
        Eligibility::RestartRequired { .. }
    ));
    let animation_roots = vec![root.join("case-sensitive-content")];
    write(
        &animation_roots[0],
        "media/AnimSets/fixture.xml",
        "native animation fixture",
    )
    .map_err(|error| error.to_string())?;
    assert!(matches!(
        activation::assess(&profile, &animation_roots, &[])?,
        Eligibility::RestartRequired { .. }
    ));
    let tile_roots = vec![root.join("native-tile-content")];
    write(
        &tile_roots[0],
        "media/FixtureTiles.tiles",
        "native tile definition fixture",
    )
    .map_err(|error| error.to_string())?;
    assert!(matches!(
        activation::assess(&profile, &tile_roots, &[])?,
        Eligibility::RestartRequired { .. }
    ));
    let tile_opt_in = Profile::parse(
        root.join("tile-profile.properties"),
        "id=NativeConsumer\nlive=client-shared\n",
    )
    .map_err(|error| error.to_string())?;
    assert_eq!(
        activation::assess(&tile_opt_in, &tile_roots, &[])?,
        Eligibility::Live {
            limitations: vec![StartupComponent::TileDefinitions]
        }
    );
    let template = Profile::parse(
        root.join("template.properties"),
        "id=NativeConsumer\nlive=client-shared\n",
    )
    .map_err(|error| error.to_string())?;
    assert_eq!(
        activation::assess(&profile, &roots, std::slice::from_ref(&template))?,
        Eligibility::Live {
            limitations: vec![StartupComponent::Definitions, StartupComponent::ServerLua]
        }
    );
    let explicit_auto = Profile::parse(
        root.join("selection.properties"),
        "id=NativeConsumer\nlive=auto\n",
    )
    .map_err(|error| error.to_string())?;
    assert!(matches!(
        activation::assess(&explicit_auto, &roots, &[template])?,
        Eligibility::RestartRequired { .. }
    ));
    let explicit_startup = Profile::parse(
        root.join("selection.properties"),
        "id=NativeConsumer\nlive=startup\n",
    )
    .map_err(|error| error.to_string())?;
    assert!(matches!(
        activation::assess(&explicit_startup, &[], &[])?,
        Eligibility::RestartRequired { .. }
    ));
    assert!(
        Profile::parse(
            root.join("selection.properties"),
            "id=NativeConsumer\nlive=anything\n"
        )
        .is_err()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let private = root.join("unreadable-startup-resource");
        write(&private, "target/definition.txt", "native fixture")
            .map_err(|error| error.to_string())?;
        let inaccessible_roots = vec![root.join("unreadable-managed-content")];
        std::fs::create_dir_all(inaccessible_roots[0].join("media"))
            .map_err(|error| error.to_string())?;
        std::os::unix::fs::symlink(
            private.join("target"),
            inaccessible_roots[0].join("media/scripts"),
        )
        .map_err(|error| error.to_string())?;
        let permissions = std::fs::metadata(&private)
            .map_err(|error| error.to_string())?
            .permissions();
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o000))
            .map_err(|error| error.to_string())?;
        let refused = activation::assess(&profile, &inaccessible_roots, &[]);
        std::fs::set_permissions(&private, permissions).map_err(|error| error.to_string())?;
        assert!(
            refused.is_err(),
            "An unreadable startup resource cannot be classified as absent"
        );
    }
    println!(
        "PASS centralized startup-resource/server eligibility, explicit policy and catalog inheritance without changing selections"
    );
    Ok(())
}

fn named(context: &mut lua::Context<'_, '_>, name: &str) -> lua::Result<lua::Value> {
    context.jni(|env| {
        let name = env.new_string(name)?;
        let value = env
            .call_static_method(
                "zombie/modding/ActiveMods",
                "getById",
                "(Ljava/lang/String;)Lzombie/modding/ActiveMods;",
                &[JValue::Object(&name)],
            )?
            .l()?;
        env.new_global_ref(value)
            .map(|value| lua::Value(Some(value)))
    })
}
fn append(
    context: &mut lua::Context<'_, '_>,
    selection: &lua::Value,
    method: &str,
    value: &str,
) -> lua::Result<()> {
    context.jni(|env| {
        let list = env
            .call_method(selection.object(), method, "()Ljava/util/ArrayList;", &[])?
            .l()?;
        let value = env.new_string(value)?;
        env.call_method(
            list,
            "add",
            "(Ljava/lang/Object;)Z",
            &[JValue::Object(&value)],
        )?;
        Ok(())
    })
}
fn strings(context: &mut lua::Context<'_, '_>, list: &lua::Value) -> lua::Result<Vec<String>> {
    context.jni(|env| {
        env.with_local_frame(16, |env| {
            let count = env.call_method(list.object(), "size", "()I", &[])?.i()?;
            let mut result = Vec::new();
            for index in 0..count {
                let value = env
                    .call_method(
                        list.object(),
                        "get",
                        "(I)Ljava/lang/Object;",
                        &[JValue::Int(index)],
                    )?
                    .l()?;
                let value = env.auto_local(jni::objects::JString::from(value));
                result.push(env.get_string(&value)?.to_string_lossy().into_owned());
            }
            Ok(result)
        })
    })
}
fn list(
    context: &mut lua::Context<'_, '_>,
    selection: &lua::Value,
    method: &str,
) -> lua::Result<Vec<String>> {
    let value = context.jni(|env| {
        let value = env
            .call_method(selection.object(), method, "()Ljava/util/ArrayList;", &[])?
            .l()?;
        env.new_global_ref(value)
            .map(|value| lua::Value(Some(value)))
    })?;
    strings(context, &value)
}
fn write_native(
    context: &mut lua::Context<'_, '_>,
    path: &Path,
    selection: &lua::Value,
) -> lua::Result<bool> {
    context.jni(|env| {
        let writer = env.new_object("zombie/modding/ActiveModsFile", "()V", &[])?;
        let path = env.new_string(path.to_string_lossy())?;
        env.call_method(
            writer,
            "write",
            "(Ljava/lang/String;Lzombie/modding/ActiveMods;)Z",
            &[JValue::Object(&path), JValue::Object(selection.object())],
        )?
        .z()
    })
}
fn assert_native_lists(
    context: &mut lua::Context<'_, '_>,
    path: &Path,
    mods: &[&str],
    maps: &[&str],
) -> lua::Result<()> {
    let parsed = context.jni(|env| {
        let name = env.new_string("verification-private")?;
        let value = env.new_object(
            "zombie/modding/ActiveMods",
            "(Ljava/lang/String;)V",
            &[JValue::Object(&name)],
        )?;
        env.new_global_ref(value)
            .map(|value| lua::Value(Some(value)))
    })?;
    let read = context.jni(|env| {
        let reader = env.new_object("zombie/modding/ActiveModsFile", "()V", &[])?;
        let path = env.new_string(path.to_string_lossy())?;
        env.call_method(
            reader,
            "read",
            "(Ljava/lang/String;Lzombie/modding/ActiveMods;)Z",
            &[JValue::Object(&path), JValue::Object(parsed.object())],
        )?
        .z()
    })?;
    assert!(read);
    assert_eq!(list(context, &parsed, "getMods")?, mods);
    assert_eq!(list(context, &parsed, "getMapOrder")?, maps);
    Ok(())
}
fn assert_unchanged_live(
    context: &mut lua::Context<'_, '_>,
    current: &lua::Value,
    active: &lua::Value,
) -> lua::Result<()> {
    assert_eq!(list(context, current, "getMods")?, ["CurrentWorldOnly"]);
    assert_eq!(strings(context, active)?, ["ServerApproved"]);
    assert!(!context.jni(|env| {
        env.call_static_method(
            "zombie/modding/ActiveMods",
            "requiresResetLua",
            "(Lzombie/modding/ActiveMods;)Z",
            &[JValue::Object(current.object())],
        )?
        .z()
    })?);
    Ok(())
}
