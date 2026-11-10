#![allow(dead_code)]
#[path = "../../../crates/luabuddy-host/src/game.rs"]
mod game;
#[path = "../../../crates/luabuddy-host/src/lua.rs"]
mod lua;
#[path = "../../../crates/luabuddy-host/src/report.rs"]
mod report;

use jni::{
    InitArgsBuilder, JNIVersion, JavaVM,
    objects::{JObject, JValue},
};
use luabuddy_core::{
    ModId, Profile,
    diagnostics::{ModDiagnostic, ModStatus},
    lifecycle::Operation,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

struct Fixture(PathBuf);
impl Fixture {
    fn create(path: PathBuf) -> std::io::Result<Self> {
        std::fs::create_dir(&path)?;
        Ok(Self(path))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod ownership_tests {
    use super::Fixture;
    #[test]
    fn refusing_an_existing_directory_preserves_its_contents() {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let parent = Fixture::create(std::env::temp_dir().join(format!(
            "luabuddy-proof-ownership-{}-{suffix}",
            std::process::id()
        )))
        .unwrap();
        let existing = parent.0.join("preexisting");
        std::fs::create_dir(&existing).unwrap();
        std::fs::write(
            existing.join("sentinel"),
            b"preserve independently owned data",
        )
        .unwrap();
        assert!(Fixture::create(existing.clone()).is_err());
        assert_eq!(
            std::fs::read(existing.join("sentinel")).unwrap(),
            b"preserve independently owned data"
        );
    }
}

fn write(root: &Path, name: &str, text: &str) -> std::io::Result<()> {
    let path = root.join(name);
    std::fs::create_dir_all(path.parent().expect("fixture file has a parent"))?;
    std::fs::write(path, text)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let game = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("Provide the installed game directory")?
        .canonicalize()?;
    let fixture_path =
        std::env::temp_dir().join(format!("luabuddy-metadata-proof-{}", std::process::id()));
    let fixture = Fixture::create(fixture_path)?;
    std::fs::create_dir(fixture.0.join("home"))?;
    std::fs::create_dir(fixture.0.join("cache"))?;
    let options = InitArgsBuilder::new()
        .version(JNIVersion::V8)
        .option(format!(
            "-Djava.class.path={}",
            game.join("projectzomboid.jar").display()
        ))
        .option(format!("-Duser.home={}", fixture.0.join("home").display()))
        .option("-Dzomboid.steam=0")
        .option("-Xcheck:jni")
        .option("-Xverify:all")
        .build()?;
    std::env::set_current_dir(&game)?;
    let vm = JavaVM::new(options)?;
    let mut env = vm.attach_current_thread()?;
    for class in [
        "zombie/core/random/RandStandard",
        "zombie/core/random/RandLua",
    ] {
        let signature = format!("L{class};");
        let instance = env.get_static_field(class, "INSTANCE", &signature)?.l()?;
        env.call_method(instance, "init", "()V", &[])?;
    }
    let filesystem = env
        .get_static_field(
            "zombie/ZomboidFileSystem",
            "instance",
            "Lzombie/ZomboidFileSystem;",
        )?
        .l()?;
    let cache = env.new_string(fixture.0.join("cache").to_string_lossy())?;
    env.call_method(
        &filesystem,
        "setCacheDir",
        "(Ljava/lang/String;)V",
        &[JValue::Object(&cache)],
    )?;
    env.call_method(&filesystem, "init", "()V", &[])?;
    let languages = env
        .get_static_field(
            "zombie/core/Languages",
            "instance",
            "Lzombie/core/Languages;",
        )?
        .l()?;
    env.call_method(&languages, "init", "()V", &[])?;
    env.call_static_method("zombie/Lua/LuaManager", "init", "()V", &[])?;
    write(
        &fixture.0,
        "mods/Library/42/mod.info",
        "name=Library\nid=Library\nversionMin=42.0\nloadModAfter=OptionalNative\n",
    )?;
    write(
        &fixture.0,
        "mods/Consumer/42/mod.info",
        "name=Consumer\nid=Consumer\nrequire=Library\nversionMin=42.0\n",
    )?;
    write(
        &fixture.0,
        "mods/Flat/mod.info",
        "name=Flat\nid=Flat\nversionMin=42.0\n",
    )?;
    write(
        &fixture.0,
        "mods/Future/42/mod.info",
        "name=Future\nid=Future\nversionMin=999.0\n",
    )?;
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        fixture.0.join("mods/Library"),
        fixture.0.join("registered-alias"),
    )?;
    #[cfg(not(unix))]
    let alias = fixture.0.join("mods/Library");
    #[cfg(unix)]
    let alias = fixture.0.join("registered-alias");
    let id = env.new_string("Library")?;
    let root = env.new_string(alias.to_string_lossy())?;
    env.call_method(
        &filesystem,
        "setModIdToDir",
        "(Ljava/lang/String;Ljava/lang/String;)V",
        &[JValue::Object(&id), JValue::Object(&root)],
    )?;
    let profiles = [
        Profile::parse(
            fixture.0.join("config/consumer.properties"),
            "id=Consumer\npath=../mods/Consumer\n",
        )?,
        Profile::parse(
            fixture.0.join("config/flat.properties"),
            "id=Flat\npath=../mods/Flat\nworkshop=3788245131\n",
        )?,
        Profile::parse(
            fixture.0.join("config/future.properties"),
            "id=Future\npath=../mods/Future\n",
        )?,
        Profile::parse(
            fixture.0.join("config/wrong.properties"),
            "id=Wrong\npath=../mods/Consumer\n",
        )?,
    ];
    let mut context = lua::Context::current(&mut env)?;
    let native = BTreeSet::from([ModId::new("Library")?]);
    let discovered = game::discover(&mut context, &profiles, &native)?;
    assert_eq!(
        discovered.metadata[&ModId::new("Consumer")?].requires,
        vec![ModId::new("Library")?]
    );
    assert_eq!(
        discovered.metadata[&ModId::new("Flat")?].roots,
        vec![fixture.0.join("mods/Flat")]
    );
    assert_eq!(
        discovered.metadata[&ModId::new("Library")?].roots[0],
        alias.join("common")
    );
    assert!(discovered.failures[&ModId::new("Future")?].contains("unavailable"));
    assert!(
        discovered.failures[&ModId::new("Wrong")?].contains("Expected Wrong but found Consumer")
    );
    let registered = Profile::parse(
        fixture.0.join("config/library.properties"),
        "id=Library\nworkshop=12345\n",
    )?;
    let again = game::discover(&mut context, &[registered], &BTreeSet::new())?;
    assert_eq!(
        again.metadata[&ModId::new("Library")?].roots[0],
        alias.join("common")
    );
    let unavailable = Profile::parse(
        fixture.0.join("config/unavailable.properties"),
        "id=Uninstalled\nworkshop=12345\n",
    )?;
    let missing = game::discover(&mut context, &[unavailable], &BTreeSet::new())?;
    assert!(missing.failures[&ModId::new("Uninstalled")?].contains("Steam is unavailable"));
    let after = game::discover(
        &mut context,
        &[Profile::parse(
            fixture.0.join("config/library.properties"),
            "id=Library\n",
        )?],
        &BTreeSet::new(),
    )?;
    assert!(after.metadata.contains_key(&ModId::new("Library")?));
    println!(
        "PASS: actual native identity, dependency metadata, registered symlink precedence, flat Workshop root, unavailable versions, no-Steam refusal and cache recovery"
    );
    write(
        &fixture.0,
        "mods/Consumer/common/media/lua/shared/Translate/EN/UI.json",
        "{\"UI_LuaBuddyRustOverlay\":\"common\",\"UI_LuaBuddyRustFallback\":\"fallback %1\"}",
    )?;
    write(
        &fixture.0,
        "mods/Consumer/42/media/lua/shared/Translate/EN/UI.json",
        "{\"UI_LuaBuddyRustOverlay\":\"version\"}",
    )?;
    write(
        &fixture.0,
        "mods/Consumer/42/media/lua/shared/Translate/DE/UI.json",
        "{\"UI_LuaBuddyRustOverlay\":\"Version DE\"}",
    )?;
    let roots = [
        fixture.0.join("mods/Consumer/common"),
        fixture.0.join("mods/Consumer/42"),
    ];
    for (language, expected) in [("EN", "version"), ("DE", "Version DE"), ("FI", "version")] {
        let language_name = context.env.new_string(language)?;
        let language = context
            .env
            .call_method(
                &languages,
                "getByName",
                "(Ljava/lang/String;)Lzombie/core/Language;",
                &[JValue::Object(&language_name)],
            )?
            .l()?;
        context.env.call_static_method(
            "zombie/core/Translator",
            "setLanguage",
            "(Lzombie/core/Language;)V",
            &[JValue::Object(&language)],
        )?;
        game::translations(&mut context, &roots)?;
        let overlay = context.env.new_string("UI_LuaBuddyRustOverlay")?;
        let args = context
            .env
            .new_object_array(0, "java/lang/Object", JObject::null())?;
        let text = context
            .env
            .call_static_method(
                "zombie/core/Translator",
                "getText",
                "(Ljava/lang/String;[Ljava/lang/Object;)Ljava/lang/String;",
                &[JValue::Object(&overlay), JValue::Object(&args)],
            )?
            .l()?;
        assert_eq!(
            context.env.get_string((&text).into())?.to_string_lossy(),
            expected
        );
        let fallback = context.env.new_string("UI_LuaBuddyRustFallback")?;
        let args = context
            .env
            .new_object_array(1, "java/lang/Object", JObject::null())?;
        let number = context
            .env
            .call_static_method(
                "java/lang/Integer",
                "valueOf",
                "(I)Ljava/lang/Integer;",
                &[JValue::Int(100)],
            )?
            .l()?;
        context.env.set_object_array_element(&args, 0, number)?;
        let text = context
            .env
            .call_static_method(
                "zombie/core/Translator",
                "getText",
                "(Ljava/lang/String;[Ljava/lang/Object;)Ljava/lang/String;",
                &[JValue::Object(&fallback), JValue::Object(&args)],
            )?
            .l()?;
        assert_eq!(
            context.env.get_string((&text).into())?.to_string_lossy(),
            "fallback 100"
        );
    }
    println!(
        "PASS: actual native translation parser, common/version overlay, English/German/fallback and native placeholder substitution"
    );
    let owned_path = game.join("media/lua/shared/defines.lua");
    let foreign_path = game.join("media/lua/shared/env.lua");
    let owned = context.compile(
        &std::fs::read_to_string(&owned_path)?,
        &owned_path.to_string_lossy(),
    )?;
    let foreign = context.compile(
        &std::fs::read_to_string(&foreign_path)?,
        &foreign_path.to_string_lossy(),
    )?;
    let events = context.global("Events")?;
    let event = context.get(&events, "OnGameBoot")?;
    let add = context.get(&event, "Add")?;
    context.call(&add, std::slice::from_ref(&owned))?;
    context.call(&add, std::slice::from_ref(&foreign))?;
    let files = BTreeSet::from([owned_path.to_string_lossy().into_owned()]);
    assert_eq!(
        game::callbacks(&mut context, "OnGameBoot", &files)?.len(),
        1
    );
    assert!(game::callback_registered(
        &mut context,
        "OnGameBoot",
        &owned
    )?);
    assert_eq!(
        game::callback_filename(&mut context, &owned)?,
        owned_path.to_string_lossy()
    );
    context.env.set_static_field(
        "se/krka/kahlua/luaj/compiler/LuaCompiler",
        (
            "se/krka/kahlua/luaj/compiler/LuaCompiler",
            "rewriteEvents",
            "Z",
        ),
        JValue::Bool(1),
    )?;
    assert!(
        game::remove_callbacks(&mut context, &files)
            .unwrap_err()
            .contains("did not remove")
    );
    assert!(game::callback_registered(
        &mut context,
        "OnGameBoot",
        &owned
    )?);
    context.env.set_static_field(
        "se/krka/kahlua/luaj/compiler/LuaCompiler",
        (
            "se/krka/kahlua/luaj/compiler/LuaCompiler",
            "rewriteEvents",
            "Z",
        ),
        JValue::Bool(0),
    )?;
    assert_eq!(game::remove_callbacks(&mut context, &files)?, 1);
    assert!(!game::callback_registered(
        &mut context,
        "OnGameBoot",
        &owned
    )?);
    assert!(game::callback_registered(
        &mut context,
        "OnGameBoot",
        &foreign
    )?);
    let remove = context.get(&event, "Remove")?;
    context.call(&remove, &[foreign])?;
    println!(
        "PASS: native by-file callback snapshots, identity membership and scoped removal preserve foreign callbacks"
    );
    let cause = context.jni(|env| {
        let message = env.new_string("metadata trace fixture")?;
        let cause = env.new_object(
            "java/io/IOException",
            "(Ljava/lang/String;)V",
            &[JValue::Object(&message)],
        )?;
        env.new_global_ref(cause)
            .map(|reference| lua::Value(Some(reference)))
    })?;
    let trace = context.java_trace(&cause)?;
    assert!(trace.contains("java.io.IOException: metadata trace fixture"));
    let plain = context.string("plain diagnostic cause")?;
    assert_eq!(context.java_trace(&plain)?, "plain diagnostic cause");
    let mut report = report::OperationReport::begin(
        &mut context,
        7,
        &Operation::Startup,
        fixture.0.join("config"),
        BTreeMap::new(),
    )?;
    report.record(
        ModId::new("Consumer")?,
        ModDiagnostic {
            status: ModStatus::Loaded,
            stage: "complete".into(),
            profile: profiles[0].file.clone(),
            source: None,
            error: None,
            traceback: None,
            java_traceback: None,
            configuration: luabuddy_core::diagnostics::ProfileDiagnostic::default(),
            source_count: 1,
            generation: 1,
        },
    );
    let first = report.finish(&mut context, None)?;
    let second = report::OperationReport::begin(
        &mut context,
        7,
        &Operation::Refresh,
        fixture.0.join("config"),
        first.mods.clone(),
    )?
    .finish(&mut context, Some("refused before mutation"))?;
    assert_eq!(first.session, second.session);
    assert_eq!(second.summary["loaded"], 1);
    assert_eq!(
        second.mods[&ModId::new("Consumer")?].status,
        ModStatus::Loaded
    );
    let saved: luabuddy_core::diagnostics::DiagnosticReport = serde_json::from_slice(
        &std::fs::read(fixture.0.join("cache/LuaBuddy/diagnostics.json"))?,
    )?;
    assert_eq!(saved.operation, "refresh");
    assert_eq!(saved.error.as_deref(), Some("refused before mutation"));
    assert!(saved.game_version.starts_with("42.22"));
    assert_eq!(
        saved.java_version,
        game::property(&mut context, "java.version")?.ok_or("Missing JVM version")?
    );
    println!(
        "PASS: real cache report, stable session, retained running records, actual game/JVM versions and operation error"
    );
    Ok(())
}
