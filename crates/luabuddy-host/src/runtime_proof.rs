use crate::lua::{Context, Value};
use jni::{JNIEnv, objects::JValue};
use std::path::{Path, PathBuf};

type Result<T> = std::result::Result<T, String>;

pub struct Fixture {
    root: PathBuf,
    evidence: Option<PathBuf>,
}

impl Fixture {
    pub fn new() -> std::io::Result<Self> {
        let root =
            std::env::temp_dir().join(format!("luabuddy-runtime-proof-{}", std::process::id()));
        std::fs::create_dir(&root)?;
        let evidence = std::env::var_os("LUABUDDY_PROOF_EVIDENCE").map(PathBuf::from);
        let fixture = Self { root, evidence };
        for name in ["home", "cache", "config", "mods"] {
            std::fs::create_dir(fixture.root.join(name))?;
        }
        if let Some(evidence) = &fixture.evidence {
            std::fs::create_dir_all(evidence)?;
        }
        Ok(fixture)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn write(&self, path: &str, text: &str) -> Result<()> {
        let path = self.root.join(path);
        std::fs::create_dir_all(path.parent().ok_or("Fixture path has no parent")?)
            .map_err(|e| e.to_string())?;
        std::fs::write(path, text).map_err(|e| e.to_string())
    }

    fn profile(&self, id: &str, settings: &str) -> Result<()> {
        self.write(
            &format!("config/{id}.properties"),
            &format!("id={id}\npath=../mods/{id}\n{settings}"),
        )
    }

    fn module(&self, id: &str, path: &str, text: &str) -> Result<()> {
        self.write(
            &format!("mods/{id}/42/mod.info"),
            &format!("name={id}\nid={id}\nversionMin=42.0\n"),
        )?;
        self.write(&format!("mods/{id}/42/media/lua/{path}"), text)
    }

    fn clear_profiles(&self) -> Result<()> {
        for entry in std::fs::read_dir(self.root.join("config")).map_err(|e| e.to_string())? {
            std::fs::remove_file(entry.map_err(|e| e.to_string())?.path())
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn snapshot(&self, name: &str) -> Result<()> {
        if let Some(evidence) = &self.evidence {
            let data = std::fs::read(self.root.join("cache/LuaBuddy/diagnostics.json"))
                .map_err(|e| e.to_string())?;
            std::fs::write(evidence.join(format!("runtime-{name}.json")), data)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn lua(context: &mut Context<'_, '_>, text: &str) -> Result<Vec<Value>> {
    let closure = context.compile(text, "LuaBuddy/runtime-proof/assertions.lua")?;
    context.call(&closure, &[])
}

fn fresh<'borrow, 'vm>(env: &'borrow mut JNIEnv<'vm>) -> Result<Context<'borrow, 'vm>> {
    env.call_static_method("zombie/Lua/LuaEventManager", "Reset", "()V", &[])
        .map_err(|e| e.to_string())?;
    env.call_static_method("zombie/Lua/LuaManager", "init", "()V", &[])
        .map_err(|e| e.to_string())?;
    let mut context = Context::current(env)?;
    lua(
        &mut context,
        "nativeMarker = {}; nativeHits = 0; require = function(name) assert(name == 'native-fixture', 'unexpected base require ' .. tostring(name)); nativeHits = nativeHits + 1; return nativeMarker end; Events.OnGameStart.Add(function() nativeStarts = (nativeStarts or 0) + 1 end)",
    )?;
    Ok(context)
}

fn trigger(context: &mut Context<'_, '_>, name: &str) -> Result<()> {
    context.jni(|env| {
        let name = env.new_string(name)?;
        env.call_static_method(
            "zombie/Lua/LuaEventManager",
            "triggerEvent",
            "(Ljava/lang/String;)V",
            &[JValue::Object(&name)],
        )?;
        Ok(())
    })
}

fn refresh(context: &mut Context<'_, '_>) -> Result<(bool, String)> {
    let state = context.global("LuaBuddy")?;
    let callback = context.get(&state, "refresh")?;
    let result = context.call(&callback, &[])?;
    if result.len() != 2 {
        return Err("Refresh must return boolean and message".into());
    }
    Ok((context.truth(&result[0])?, context.text(&result[1])?))
}

fn reload(context: &mut Context<'_, '_>, id: &str) -> Result<(bool, String)> {
    let state = context.global("LuaBuddy")?;
    let callback = context.get(&state, "reload")?;
    let id = context.string(id)?;
    let result = context.call(&callback, &[id])?;
    if result.len() != 2 {
        return Err("Reload must return boolean and message".into());
    }
    Ok((context.truth(&result[0])?, context.text(&result[1])?))
}

fn report(fixture: &Fixture) -> Result<serde_json::Value> {
    serde_json::from_slice(
        &std::fs::read(fixture.root.join("cache/LuaBuddy/diagnostics.json"))
            .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

pub fn run(env: &mut JNIEnv<'_>, fixture: &Fixture) -> Result<()> {
    for class in [
        "zombie/core/random/RandStandard",
        "zombie/core/random/RandLua",
    ] {
        let instance = env
            .get_static_field(class, "INSTANCE", format!("L{class};"))
            .and_then(|v| v.l())
            .map_err(|e| e.to_string())?;
        env.call_method(instance, "init", "()V", &[])
            .map_err(|e| e.to_string())?;
    }
    let filesystem = env
        .get_static_field(
            "zombie/ZomboidFileSystem",
            "instance",
            "Lzombie/ZomboidFileSystem;",
        )
        .and_then(|v| v.l())
        .map_err(|e| e.to_string())?;
    let cache = env
        .new_string(fixture.root.join("cache").to_string_lossy())
        .map_err(|e| e.to_string())?;
    env.call_method(
        &filesystem,
        "setCacheDir",
        "(Ljava/lang/String;)V",
        &[JValue::Object(&cache)],
    )
    .map_err(|e| e.to_string())?;
    env.call_method(&filesystem, "init", "()V", &[])
        .map_err(|e| e.to_string())?;
    let languages = env
        .get_static_field(
            "zombie/core/Languages",
            "instance",
            "Lzombie/core/Languages;",
        )
        .and_then(|v| v.l())
        .map_err(|e| e.to_string())?;
    env.call_method(languages, "init", "()V", &[])
        .map_err(|e| e.to_string())?;
    let server = env
        .get_static_field_id("zombie/network/GameServer", "server", "Z")
        .map_err(|e| e.to_string())?;
    env.set_static_field("zombie/network/GameServer", server, JValue::Bool(0))
        .map_err(|e| e.to_string())?;
    let client = env
        .get_static_field_id("zombie/network/GameClient", "client", "Z")
        .map_err(|e| e.to_string())?;
    env.set_static_field("zombie/network/GameClient", client, JValue::Bool(1))
        .map_err(|e| e.to_string())?;
    let checks = [
        (
            "baseline",
            baseline as fn(&mut JNIEnv<'_>, &Fixture) -> Result<()>,
        ),
        ("compile failure", compile_failure),
        ("cooperative reload", cooperative_reload),
        ("cleanup quarantine", cleanup_failure),
        ("replay failure", replay_failure),
        (
            "configuration preflight refusal",
            configuration_preflight_refusal,
        ),
        ("lifecycle guards", lifecycle_guards),
        ("runtime rejection", runtime_rejection),
        ("replay ordering", replay_ordering),
        ("cycles and failures", cycles_and_failures),
        ("dependency and configuration failures", dependency_errors),
        ("native ordering", native_ordering),
        ("early UI timing", early_ui_timing),
        ("namespace ownership", namespace_ownership),
        ("nested startup readiness", nested_startup_readiness),
        (
            "native startup eligibility transitions",
            startup_eligibility_transitions,
        ),
        ("renamed disabled selection", renamed_disabled_selection),
        (
            "profile scan order and provider precedence",
            profile_scan_order,
        ),
        ("server isolation", server_isolation),
    ];
    let groups = checks.len();
    let mut failures = Vec::new();
    for (name, check) in checks {
        if let Err(error) = check(env, fixture) {
            eprintln!("FAIL {name}: {error}");
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_describe();
                let _ = env.exception_clear();
            }
            failures.push(format!("{name}: {error}"));
        }
    }
    if failures.is_empty() {
        println!("PASS {groups} production runtime scenario groups");
        Ok(())
    } else {
        Err(failures.join("\n"))
    }
}

fn cooperative_source(version: i32) -> String {
    format!(
        "local object = {{ version = {version}, ticks = 0 }}; Coop = object; coopRuns = (coopRuns or 0) + 1; Events.OnTick.Add(function() object.ticks = object.ticks + 1 end); object.cleanup = function(context) cleanupAttempts = (cleanupAttempts or 0) + 1; assert(context.id == 'Coop'); assert(context.runtime == {}, 'original activation runtime'); assert(context.operation == '{}', 'original activation operation'); assert(context.state == LuaBuddy and context.active.Coop); if cleanupMode == 'refuse' then return false, 'busy fixture' end; if cleanupMode == 'throw' then error('cleanup fixture failed') end; if cleanupMode == 'nil' then return nil end; cleanupCalls = (cleanupCalls or 0) + 1; lastCleanupContext = context; Coop = nil; return true end; return object",
        if version == 1 { "false" } else { "true" },
        if version == 1 { "startup" } else { "reload" }
    )
}

fn cooperative_reload(env: &mut JNIEnv<'_>, fixture: &Fixture) -> Result<()> {
    fixture.clear_profiles()?;
    fixture.module("Coop", "shared/Coop.lua", &cooperative_source(1))?;
    fixture.profile("Coop", "cleanup=Coop.cleanup\n")?;
    let mut context = fresh(env)?;
    lua(
        &mut context,
        "Events.OnTick.Add(function() foreignTicks = (foreignTicks or 0) + 1 end)",
    )?;
    trigger(&mut context, "OnGameStart")?;
    lua(
        &mut context,
        "assert(LuaBuddy.mods.Coop == 'loaded', LuaBuddy.mods.Coop); oldCoop = require('Coop'); retainedRequire = require; assert(oldCoop.version == 1 and coopRuns == 1)",
    )?;
    trigger(&mut context, "OnTick")?;
    lua(
        &mut context,
        "assert(oldCoop.ticks == 1 and foreignTicks == 1)",
    )?;
    fixture.module("Coop", "shared/Coop.lua", &cooperative_source(2))?;
    let (ok, reason) = reload(&mut context, "Coop")?;
    if !ok {
        return Err(format!("Cooperative reload failed: {reason}"));
    }
    lua(
        &mut context,
        "assert(Coop.version == 2 and Coop ~= oldCoop and require('Coop') == Coop); assert(require == retainedRequire and coopRuns == 2 and cleanupCalls == 1 and cleanupAttempts == 1); assert(lastCleanupContext.runtime == false and lastCleanupContext.operation == 'startup')",
    )?;
    let loaded_record = report(fixture)?["mods"]["Coop"].clone();
    fixture.snapshot("cooperative-running")?;
    trigger(&mut context, "OnTick")?;
    lua(
        &mut context,
        "assert(oldCoop.ticks == 1 and Coop.ticks == 1 and foreignTicks == 2, 'owned callbacks must be replaced while foreign callbacks remain'); cleanupMode = 'refuse'; savedCoop = Coop",
    )?;
    fixture.module("Coop", "shared/Coop.lua", &cooperative_source(3))?;
    fixture.profile("Coop", "cleanup=Coop.cleanup\nreplay=OnLoad\n")?;
    fixture.module(
        "Coop",
        "client/CandidateOnly.lua",
        "candidateOnlyRan = true",
    )?;
    let (ok, reason) = reload(&mut context, "Coop")?;
    if ok || !reason.contains("busy fixture") {
        return Err(format!("Cleanup refusal was not preserved: {ok} {reason}"));
    }
    lua(
        &mut context,
        "assert(Coop == savedCoop and require('Coop') == savedCoop and coopRuns == 2 and cleanupCalls == 1 and cleanupAttempts == 2 and LuaBuddy.mods.Coop == 'loaded' and not candidateOnlyRan); cleanupMode = nil",
    )?;
    fixture.snapshot("cooperative-refused")?;
    if report(fixture)?["mods"]["Coop"] != loaded_record {
        return Err(format!(
            "Cleanup refusal published rejected candidate metadata over the loaded generation report; previous={loaded_record}; current={}",
            report(fixture)?["mods"]["Coop"]
        ));
    }
    fixture.module("Coop", "shared/Coop.lua", "return )")?;
    let (ok, _) = reload(&mut context, "Coop")?;
    if ok {
        return Err("Syntax-invalid replacement was accepted".into());
    }
    lua(
        &mut context,
        "assert(Coop == savedCoop and require('Coop') == savedCoop and coopRuns == 2 and cleanupCalls == 1 and cleanupAttempts == 2 and LuaBuddy.mods.Coop == 'loaded')",
    )?;
    if report(fixture)?["mods"]["Coop"] != loaded_record {
        return Err(
            "Compile rejection published candidate metadata over the loaded generation report"
                .into(),
        );
    }
    fixture.module("Coop", "shared/Coop.lua", &cooperative_source(3))?;
    let (ok, reason) = reload(&mut context, "Coop")?;
    if !ok {
        return Err(format!(
            "Valid replacement after preflight refusal failed: {reason}"
        ));
    }
    lua(
        &mut context,
        "assert(Coop.version == 3 and coopRuns == 3 and cleanupCalls == 2 and cleanupAttempts == 3); assert(lastCleanupContext.runtime == true and lastCleanupContext.operation == 'reload')",
    )?;
    fixture.module(
        "CoopDependent",
        "client/CoopDependent.lua",
        "assert(require('Coop').version == 3); coopDependentRuns = (coopDependentRuns or 0) + 1",
    )?;
    fixture.profile("CoopDependent", "requires=Coop\n")?;
    let (ok, reason) = refresh(&mut context)?;
    if !ok {
        return Err(format!("Dependent activation failed: {reason}"));
    }
    let (ok, reason) = reload(&mut context, "Coop")?;
    if ok || !reason.contains("depends") {
        return Err(format!(
            "Active dependent did not refuse reload: {ok} {reason}"
        ));
    }
    lua(
        &mut context,
        "assert(Coop.version == 3 and coopRuns == 3 and cleanupAttempts == 3 and coopDependentRuns == 1)",
    )?;
    println!(
        "PASS cooperative reload, exact original cleanup context, scoped callback removal, foreign callback preservation, refusal, compile-before-cleanup and active-dependent rejection"
    );
    fixture.snapshot("cooperative-reload")?;
    Ok(())
}

fn cleanup_failure(env: &mut JNIEnv<'_>, fixture: &Fixture) -> Result<()> {
    for mode in ["throw", "nil", "activation"] {
        fixture.clear_profiles()?;
        fixture.module("Coop", "shared/Coop.lua", &cooperative_source(1))?;
        fixture.profile("Coop", "cleanup=Coop.cleanup\n")?;
        let mut context = fresh(env)?;
        trigger(&mut context, "OnGameStart")?;
        if mode == "activation" {
            fixture.module(
                "Coop",
                "shared/Coop.lua",
                "coopRuns = coopRuns + 1; error('replacement activation failed')",
            )?;
        } else {
            fixture.module("Coop", "shared/Coop.lua", &cooperative_source(2))?;
            lua(&mut context, &format!("cleanupMode = '{mode}'"))?;
        }
        let (ok, reason) = reload(&mut context, "Coop")?;
        if ok {
            return Err(format!(
                "Uncertain {mode} cleanup/activation was accepted: {reason}"
            ));
        }
        lua(
            &mut context,
            "assert(cleanupAttempts == 1); assert(LuaBuddy.mods.Coop:find('failed:', 1, true) == 1); assert(not pcall(require, 'Coop'), 'quarantined module remained callable'); savedRuns = coopRuns",
        )?;
        fixture.snapshot(&format!("cleanup-{mode}"))?;
        let (ok, _) = reload(&mut context, "Coop")?;
        if ok {
            return Err(format!("Quarantined {mode} generation retried"));
        }
        let _ = refresh(&mut context)?;
        lua(
            &mut context,
            "assert(cleanupAttempts == 1 and coopRuns == savedRuns)",
        )?;
    }
    println!(
        "PASS cleanup exception/nil and replacement activation quarantine with no repeated cleanup or execution"
    );
    Ok(())
}

fn replay_failure(env: &mut JNIEnv<'_>, fixture: &Fixture) -> Result<()> {
    fixture.clear_profiles()?;
    let mut context = fresh(env)?;
    trigger(&mut context, "OnGameStart")?;
    fixture.module("ReplayBase", "client/ReplayBase.lua", "replayBaseRuns = (replayBaseRuns or 0) + 1; Events.OnGameStart.Add(function() error('scoped replay failed') end)")?;
    fixture.module(
        "ReplayDependent",
        "client/ReplayDependent.lua",
        "replayDependentRuns = (replayDependentRuns or 0) + 1; Events.OnGameStart.Add(function() dependentReplayRan = true end)",
    )?;
    fixture.module(
        "ReplayTransitive",
        "client/ReplayTransitive.lua",
        "Events.OnLoad.Add(function() transitiveReplayRan = true end)",
    )?;
    fixture.module(
        "ReplayUnrelated",
        "client/ReplayUnrelated.lua",
        "replayUnrelatedRuns = (replayUnrelatedRuns or 0) + 1",
    )?;
    fixture.profile("ReplayBase", "replay=OnGameStart\n")?;
    fixture.profile(
        "ReplayDependent",
        "requires=ReplayBase\nreplay=OnGameStart\n",
    )?;
    fixture.profile(
        "ReplayTransitive",
        "requires=ReplayDependent\nreplay=OnLoad\n",
    )?;
    fixture.profile("ReplayUnrelated", "")?;
    let (ok, _) = refresh(&mut context)?;
    if ok {
        return Err("Scoped startup replay failure was reported successful".into());
    }
    lua(
        &mut context,
        "assert(LuaBuddy.mods.ReplayBase:find('failed:', 1, true) == 1, 'replay owner must fail'); assert(LuaBuddy.mods.ReplayDependent:find('failed:', 1, true) == 1, 'replay dependent must fail'); assert(LuaBuddy.mods.ReplayTransitive:find('failed:', 1, true) == 1, 'transitive replay dependent must fail'); assert(not dependentReplayRan and not transitiveReplayRan, 'dependent replay ran after required callback failed'); assert(LuaBuddy.mods.ReplayUnrelated == 'loaded'); assert(replayBaseRuns == 1 and replayDependentRuns == 1 and replayUnrelatedRuns == 1); assert(nativeStarts == 1)",
    )?;
    println!("PASS scoped replay owner/dependent failure attribution and unrelated continuation");
    fixture.snapshot("replay-failure")?;
    Ok(())
}

fn configuration_preflight_refusal(env: &mut JNIEnv<'_>, fixture: &Fixture) -> Result<()> {
    fixture.clear_profiles()?;
    fixture.module(
        "ConfigurationResident",
        "client/ConfigurationResident.lua",
        "configurationResidentRuns = (configurationResidentRuns or 0) + 1; Events.OnTick.Add(function() configurationResidentTicks = (configurationResidentTicks or 0) + 1 end)",
    )?;
    fixture.profile("ConfigurationResident", "")?;
    let mut context = fresh(env)?;
    trigger(&mut context, "OnGameStart")?;
    trigger(&mut context, "OnTick")?;
    lua(
        &mut context,
        "assert(LuaBuddy.mods.ConfigurationResident == 'loaded' and configurationResidentRuns == 1 and configurationResidentTicks == 1); heldConfigurationNamespace = LuaBuddy",
    )?;
    let loaded_record = report(fixture)?["mods"]["ConfigurationResident"].clone();
    fixture.module(
        "UnsupportedOption",
        "client/UnsupportedOption.lua",
        "unsupportedOptionRuns = (unsupportedOptionRuns or 0) + 1",
    )?;
    fixture.module(
        "ConfigurationNew",
        "client/ConfigurationNew.lua",
        "configurationNewRuns = (configurationNewRuns or 0) + 1",
    )?;
    fixture.profile("UnsupportedOption", "unexpectedOption=true\n")?;
    fixture.profile("ConfigurationNew", "")?;
    let (ok, reason) = refresh(&mut context)?;
    if ok
        || !reason.contains("Unknown profile option unexpectedOption")
        || !reason.contains("UnsupportedOption.properties")
    {
        return Err(format!(
            "Unsupported profile option did not refuse before mutation: {ok} {reason}"
        ));
    }
    trigger(&mut context, "OnTick")?;
    lua(
        &mut context,
        "assert(LuaBuddy == heldConfigurationNamespace); assert(LuaBuddy.mods.ConfigurationResident == 'loaded' and configurationResidentRuns == 1 and configurationResidentTicks == 2); assert(not unsupportedOptionRuns and not configurationNewRuns and LuaBuddy.mods.ConfigurationNew == nil)",
    )?;
    if report(fixture)?["mods"]["ConfigurationResident"] != loaded_record {
        return Err("Invalid profile changed an unrelated loaded generation's diagnostics".into());
    }
    fixture.snapshot("configuration-preflight-refused")?;
    fixture.profile("UnsupportedOption", "")?;
    let (ok, reason) = refresh(&mut context)?;
    if !ok {
        return Err(format!(
            "Corrected profile did not resume pending loads: {reason}"
        ));
    }
    trigger(&mut context, "OnTick")?;
    lua(
        &mut context,
        "assert(LuaBuddy == heldConfigurationNamespace); assert(LuaBuddy.mods.ConfigurationResident == 'loaded' and configurationResidentRuns == 1 and configurationResidentTicks == 3); assert(LuaBuddy.mods.UnsupportedOption == 'loaded' and unsupportedOptionRuns == 1); assert(LuaBuddy.mods.ConfigurationNew == 'loaded' and configurationNewRuns == 1)",
    )?;
    println!(
        "PASS unsupported profile preflight, preserved unrelated generation and callbacks, and corrected pending loads"
    );
    fixture.snapshot("configuration-preflight-recovered")?;
    Ok(())
}

fn baseline(env: &mut JNIEnv<'_>, fixture: &Fixture) -> Result<()> {
    fixture.module(
        "Library",
        "shared/Fixture/Library.lua",
        "libraryShared = (libraryShared or 0) + 1; return { number = 41 }",
    )?;
    fixture.module(
        "Library",
        "client/Fixture/Ready.lua",
        "libraryClient = (libraryClient or 0) + 1; libraryReady = true",
    )?;
    fixture.module("Consumer", "shared/Fixture/Overlay.lua", "return 42")?;
    fixture.write(
        "mods/Consumer/common/media/lua/shared/Fixture/Overlay.lua",
        "error('version source must override common')",
    )?;
    fixture.module(
        "Consumer",
        "shared/Fixture/Phase.lua",
        "phaseCount = (phaseCount or 0) + 1; return 'shared'",
    )?;
    fixture.module(
        "Consumer",
        "client/Fixture/Phase.lua",
        "phaseCount = phaseCount + 1; return 'client'",
    )?;
    fixture.module(
        "Consumer",
        "shared/Fixture/Nil.lua",
        "nilCount = (nilCount or 0) + 1; return nil, 'ignored return'",
    )?;
    fixture.module(
        "Consumer",
        "shared/Fixture/False.LUA",
        "falseCount = (falseCount or 0) + 1; return false, 42",
    )?;
    fixture.module(
        "Consumer",
        "shared/Fixture/Closure.lua",
        "return function() return 9 end",
    )?;
    fixture.module("Consumer", "client/Fixture/Consumer.lua", "assert(libraryReady, 'native required order'); assert(require('Fixture/Library').number == 41); consumerRuns = (consumerRuns or 0) + 1; lateImport = function() return require('Fixture/Overlay.lua') end; makeNested = function() return function() return 9 end end; nestedCallback = makeNested(); assert(require('native-fixture') == nativeMarker)")?;
    fixture.write(
        "mods/Consumer/42/mod.info",
        "name=Consumer\nid=Consumer\nrequire=Library\nversionMin=42.0\n",
    )?;
    fixture.module(
        "First",
        "shared/Fixture/Collision.lua",
        "firstRuns = (firstRuns or 0) + 1; firstObject = {}; return firstObject",
    )?;
    fixture.module(
        "Last",
        "shared/Fixture/Collision.lua",
        "lastRuns = (lastRuns or 0) + 1; lastObject = {}; return lastObject",
    )?;
    fixture.profile("Library", "")?;
    fixture.profile("Consumer", "")?;
    fixture.profile("First", "")?;
    fixture.profile("Last", "after=First\n")?;
    let mut context = fresh(env)?;
    trigger(&mut context, "OnGameStart")?;
    for name in ["lateImport", "makeNested", "nestedCallback"] {
        let closure = context.global(name)?;
        let filename = context.jni(|env| {
            let prototype = env
                .get_field(
                    closure.object(),
                    "prototype",
                    "Lse/krka/kahlua/vm/Prototype;",
                )?
                .l()?;
            let filename = env
                .get_field(prototype, "filename", "Ljava/lang/String;")?
                .l()?;
            env.new_global_ref(filename).map(|value| Value(Some(value)))
        })?;
        if context.text(&filename)?
            != fixture
                .root
                .join("mods/Consumer/42/media/lua/client/Fixture/Consumer.lua")
                .to_string_lossy()
        {
            return Err(format!(
                "Nested closure {name} lost native source filename metadata"
            ));
        }
    }
    lua(
        &mut context,
        "assert(LuaBuddy, 'production lifecycle did not initialize'); assert(LuaBuddy.ready == true, 'actual dispatcher completion was not observed'); assert(LuaBuddy.mods.Library == 'loaded', LuaBuddy.mods.Library); assert(LuaBuddy.mods.Consumer == 'loaded', LuaBuddy.mods.Consumer); assert(lateImport() == 42); assert(phaseCount == 2); assert(require('Fixture/Phase') == 'shared'); assert(require('Fixture/Nil') == nil and require('FIXTURE/NIL.LUA') == nil); assert(require('Fixture/False') == false and require('FIXTURE/FALSE.LUA') == false); assert(require('Fixture/Closure') == require('FIXTURE/CLOSURE.LUA') and require('Fixture/Closure')() == 9); assert(nilCount == 1 and falseCount == 1); assert(firstRuns == 1 and lastRuns == 1, 'shadowed files must each execute'); assert(require('Fixture/Collision') == lastObject, 'last ordered provider must win'); retainedRequire = require; retainedLibrary = require('Fixture/Library'); retainedState = LuaBuddy",
    )?;
    let (ok, message) = refresh(&mut context)?;
    if !ok {
        return Err(format!("No-op refresh failed: {message}"));
    }
    lua(
        &mut context,
        "assert(require == retainedRequire and LuaBuddy == retainedState); assert(require('Fixture/Library') == retainedLibrary); assert(consumerRuns == 1 and phaseCount == 2 and firstRuns == 1 and lastRuns == 1)",
    )?;
    trigger(&mut context, "OnGameStart")?;
    lua(
        &mut context,
        "assert(require == retainedRequire and LuaBuddy == retainedState and consumerRuns == 1 and nilCount == 1 and falseCount == 1 and nativeStarts == 2); assert(require('Fixture\\\\Library') == retainedLibrary); assert(require('shared/Fixture/Phase.lua') == 'shared' and require('client/Fixture/Phase.lua') == 'client')",
    )?;
    let report: serde_json::Value = serde_json::from_slice(
        &std::fs::read(fixture.root.join("cache/LuaBuddy/diagnostics.json"))
            .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    if report["summary"]["loaded"] != 4 {
        return Err(format!(
            "Expected four loaded profiles in actual report, got {}",
            report["summary"]
        ));
    }
    println!(
        "PASS production native lifecycle, dependency phases, overlays, shadowed-source execution, cached nil/false, aliases, native delegation, identity, no-op refresh and diagnostics"
    );
    fixture.snapshot("baseline")?;
    fixture.module("Added", "client/Added.lua", "addedRuns = (addedRuns or 0) + 1; Events.OnGameStart.Add(function() addedStarts = (addedStarts or 0) + 1 end)")?;
    fixture.profile("Added", "replay=OnGameStart\n")?;
    let (ok, message) = refresh(&mut context)?;
    if !ok {
        return Err(format!("Runtime addition failed: {message}"));
    }
    lua(
        &mut context,
        "assert(LuaBuddy.mods.Added == 'loaded', LuaBuddy.mods.Added); assert(addedRuns == 1 and addedStarts == 1); assert(nativeStarts == 2, 'foreign startup callback replayed'); assert(require == retainedRequire and LuaBuddy == retainedState); assert(consumerRuns == 1)",
    )?;
    let (ok, message) = refresh(&mut context)?;
    if !ok {
        return Err(format!("Repeated refresh failed: {message}"));
    }
    lua(
        &mut context,
        "assert(addedRuns == 1 and addedStarts == 1 and consumerRuns == 1)",
    )?;
    let old_require = context.global("require")?;
    let old_state = context.global("LuaBuddy")?;
    drop(context);
    let mut context = fresh(env)?;
    trigger(&mut context, "OnGameStart")?;
    let current = context.global("LuaBuddy")?;
    if context.same(&old_state, &current)? {
        return Err("Reconnect reused retired environment state".into());
    }
    let name = context.string("Fixture/Library")?;
    let error = context
        .call(&old_require, &[name])
        .expect_err("Retired captured require must refuse");
    if !error.contains("no longer active") {
        return Err(format!("Retired resolver lacks precise error: {error}"));
    }
    lua(
        &mut context,
        "assert(consumerRuns == 1 and phaseCount == 2 and addedRuns == 1 and addedStarts == 1)",
    )?;
    println!(
        "PASS runtime addition, scoped startup replay, repeated no-op refresh and reconnect with retired resolver refusal"
    );
    fixture.snapshot("reconnect")?;
    Ok(())
}

fn compile_failure(env: &mut JNIEnv<'_>, fixture: &Fixture) -> Result<()> {
    fixture.clear_profiles()?;
    fixture.module(
        "CompileFailure",
        "shared/Broken.lua",
        "this is not valid Lua !!!",
    )?;
    fixture.module("Dependent", "client/Dependent.lua", "dependentRan = true")?;
    fixture.module("Unrelated", "client/Unrelated.lua", "unrelatedRan = true")?;
    fixture.profile("CompileFailure", "")?;
    fixture.profile("Dependent", "requires=CompileFailure\n")?;
    fixture.profile("Unrelated", "")?;
    let mut context = fresh(env)?;
    trigger(&mut context, "OnGameStart")?;
    lua(
        &mut context,
        "assert(LuaBuddy.mods.CompileFailure:find('failed', 1, true)); assert(LuaBuddy.mods.Dependent and LuaBuddy.mods.Dependent:find('failed', 1, true), 'compile failure must fail dependent'); assert(not dependentRan); assert(LuaBuddy.mods.Unrelated == 'loaded' and unrelatedRan, 'compile failure must not block unrelated profile')",
    )?;
    println!("PASS compile failure blocks dependents and preserves unrelated activation");
    let (ok, _) = refresh(&mut context)?;
    if ok {
        return Err(
            "Refresh reported success while failed profiles remain in the current session".into(),
        );
    }
    lua(&mut context, "assert(unrelatedRan and not dependentRan)")?;
    Ok(())
}

fn lifecycle_guards(env: &mut JNIEnv<'_>, fixture: &Fixture) -> Result<()> {
    fixture.clear_profiles()?;
    fixture.module("Guard", "client/Guard.lua", "guardRuns = (guardRuns or 0) + 1; Events.OnGameStart.Add(function() startupRequest, startupReason = LuaBuddy.refresh() end)")?;
    fixture.profile("Guard", "")?;
    let mut context = fresh(env)?;
    trigger(&mut context, "OnGameStart")?;
    lua(
        &mut context,
        "assert(startupRequest == false and startupReason:find('startup', 1, true)); assert(LuaBuddy.ready and guardRuns == 1)",
    )?;
    let state = context.global("LuaBuddy")?;
    let callback = context.get(&state, "refresh")?;
    let vm = context.env.get_java_vm().map_err(|e| e.to_string())?;
    let result = std::thread::spawn(move || -> Result<(bool, String)> {
        let mut env = vm.attach_current_thread().map_err(|e| e.to_string())?;
        let mut context = Context::current(&mut env)?;
        let values = context.call(&callback, &[])?;
        if values.len() != 2 {
            return Err("Foreign-thread refresh must return a refusal pair".into());
        }
        Ok((context.truth(&values[0])?, context.text(&values[1])?))
    })
    .join()
    .map_err(|_| "Foreign-thread proof panicked")??;
    if result.0 || !result.1.contains("game thread") {
        return Err(format!(
            "Foreign-thread refresh failed to refuse: {result:?}"
        ));
    }
    fixture.module("Reentrant", "client/Reentrant.lua", "reentrantRequest, reentrantReason = LuaBuddy.refresh(); reentrantRuns = (reentrantRuns or 0) + 1")?;
    fixture.profile("Reentrant", "")?;
    let (ok, reason) = refresh(&mut context)?;
    if !ok {
        return Err(format!(
            "Outer refresh failed during safe reentrant refusal: {reason}"
        ));
    }
    lua(
        &mut context,
        "assert(reentrantRequest == false and reentrantReason:find('already running', 1, true)); assert(reentrantRuns == 1 and guardRuns == 1)",
    )?;
    fixture.profile("Guard", "enabled=false\n")?;
    let (ok, reason) = refresh(&mut context)?;
    if !ok {
        return Err(format!(
            "Disabling selection should retain current generation: {reason}"
        ));
    }
    lua(
        &mut context,
        "assert(LuaBuddy.mods.Guard == 'loaded' and guardRuns == 1)",
    )?;
    println!(
        "PASS startup completion, foreign-thread and reentrant guards plus disable-without-unload semantics"
    );
    Ok(())
}

fn runtime_rejection(env: &mut JNIEnv<'_>, fixture: &Fixture) -> Result<()> {
    let mut failures = Vec::new();
    for rule in ["before", "reverse", "path", "alias"] {
        let result = (|| -> Result<()> {
            fixture.clear_profiles()?;
            fixture.module(
                "RuntimeLibrary",
                "shared/Runtime/Library.lua",
                "libraryRuns = (libraryRuns or 0) + 1; return {}",
            )?;
            fixture.module(
                "RuntimeLibrary",
                "client/Runtime/ClientOnly.lua",
                "return {}",
            )?;
            fixture.profile(
                "RuntimeLibrary",
                if rule == "reverse" {
                    "after=RuntimeBlocked\n"
                } else {
                    ""
                },
            )?;
            let mut context = fresh(env)?;
            trigger(&mut context, "OnGameStart")?;
            lua(
                &mut context,
                "libraryObject = require('Runtime/Library'); savedRequire = require",
            )?;
            let path = match rule {
                "path" => "shared/Runtime/Library.lua",
                "alias" => "shared/Runtime/ClientOnly.lua",
                _ => "client/Runtime/Blocked.lua",
            };
            fixture.module("RuntimeBlocked", path, "blockedRuntimeRan = true")?;
            fixture.profile(
                "RuntimeBlocked",
                if rule == "before" {
                    "before=RuntimeLibrary\n"
                } else {
                    ""
                },
            )?;
            fixture.module(
                "RuntimeSafe",
                "client/RuntimeSafe.lua",
                "runtimeSafeRuns = (runtimeSafeRuns or 0) + 1",
            )?;
            fixture.profile("RuntimeSafe", "")?;
            let (ok, reason) = refresh(&mut context)?;
            if ok {
                return Err(format!("Runtime {rule} conflict was accepted: {reason}"));
            }
            lua(
                &mut context,
                "assert(not blockedRuntimeRan); assert(libraryRuns == 1 and require == savedRequire and require('Runtime/Library') == libraryObject); assert(LuaBuddy.mods.RuntimeLibrary == 'loaded'); assert(LuaBuddy.mods.RuntimeBlocked and LuaBuddy.mods.RuntimeBlocked:find('failed:', 1, true) == 1, 'rejected new profile must have failed diagnostics'); assert(LuaBuddy.mods.RuntimeSafe == 'loaded' and runtimeSafeRuns == 1, 'preflight rejection blocked unrelated new profile')",
            )?;
            Ok(())
        })();
        if let Err(error) = result {
            failures.push(format!("{rule}: {error}"));
        }
    }
    if !failures.is_empty() {
        return Err(failures.join("\n"));
    }
    println!("PASS runtime before/after, path and alias replacement rejection before mutation");
    Ok(())
}

fn replay_ordering(env: &mut JNIEnv<'_>, fixture: &Fixture) -> Result<()> {
    fixture.clear_profiles()?;
    let mut context = fresh(env)?;
    trigger(&mut context, "OnGameStart")?;
    fixture.module("ReplayOrderA", "client/ReplayOrderA.lua", "local removed = function() removedCallbackRan = true end; Events.OnGameStart.Add(function() assert(orderBReady, 'startup replay preceded later client phase'); replaySequence = (replaySequence or '') .. 'start'; Events.OnGameStart.Remove(removed) end); Events.OnGameStart.Add(removed); Events.OnLoad.Add(function() assert(replaySequence == 'start'); replaySequence = replaySequence .. ',load' end); Events.OnGameBoot.Add(function() bootSawClient = orderBReady == true end)")?;
    fixture.module(
        "ReplayOrderB",
        "client/ReplayOrderB.lua",
        "orderBReady = true",
    )?;
    fixture.profile(
        "ReplayOrderA",
        "replay=OnGameBoot;OnGameStart;OnLoad\nbefore=ReplayOrderB\n",
    )?;
    fixture.profile("ReplayOrderB", "")?;
    let (ok, reason) = refresh(&mut context)?;
    if !ok {
        return Err(format!("Ordered scoped replay failed: {reason}"));
    }
    lua(
        &mut context,
        "assert(bootSawClient == false and replaySequence == 'start,load'); assert(not removedCallbackRan, 'callback removed after snapshot was invoked'); assert(nativeStarts == 1)",
    )?;
    println!(
        "PASS scoped boot/start/load ordering, all-client completion and removed-callback snapshot isolation"
    );
    Ok(())
}

fn cycles_and_failures(env: &mut JNIEnv<'_>, fixture: &Fixture) -> Result<()> {
    fixture.clear_profiles()?;
    fixture.module(
        "Cycle",
        "shared/Cycle/A.lua",
        "cycleRuns = (cycleRuns or 0) + 1; return require('Cycle/B')",
    )?;
    fixture.module("Cycle", "shared/Cycle/B.lua", "return require('Cycle/A')")?;
    fixture.module(
        "ExecutionFailure",
        "client/ExecutionFailure.lua",
        "executionRuns = (executionRuns or 0) + 1; error('execution fixture failed')",
    )?;
    fixture.module(
        "CycleUnrelated",
        "client/CycleUnrelated.lua",
        "cycleUnrelatedRuns = (cycleUnrelatedRuns or 0) + 1",
    )?;
    for id in ["Cycle", "ExecutionFailure", "CycleUnrelated"] {
        fixture.profile(id, "")?;
    }
    let mut context = fresh(env)?;
    trigger(&mut context, "OnGameStart")?;
    lua(
        &mut context,
        "assert(LuaBuddy.mods.Cycle:find('Circular Lua dependency', 1, true)); assert(LuaBuddy.mods.ExecutionFailure:find('execution fixture failed', 1, true)); assert(LuaBuddy.mods.CycleUnrelated == 'loaded' and cycleUnrelatedRuns == 1); assert(cycleRuns == 1 and executionRuns == 1); assert(not pcall(require, 'Cycle/A')); assert(cycleRuns == 1)",
    )?;
    let (ok, _) = refresh(&mut context)?;
    if ok {
        return Err("Repeated refresh hid failed cycle/runtime generations".into());
    }
    lua(
        &mut context,
        "assert(cycleRuns == 1 and executionRuns == 1 and cycleUnrelatedRuns == 1)",
    )?;
    println!(
        "PASS circular imports, execution failure, unrelated continuation and no retry in the same generation"
    );
    Ok(())
}

fn server_isolation(env: &mut JNIEnv<'_>, fixture: &Fixture) -> Result<()> {
    fixture.clear_profiles()?;
    fixture.module("Isolation", "client/Isolation.lua", "isolationRan = true")?;
    fixture.profile("Isolation", "")?;
    let server = env
        .get_static_field_id("zombie/network/GameServer", "server", "Z")
        .map_err(|e| e.to_string())?;
    let client = env
        .get_static_field_id("zombie/network/GameClient", "client", "Z")
        .map_err(|e| e.to_string())?;
    env.set_static_field("zombie/network/GameServer", server, JValue::Bool(1))
        .map_err(|e| e.to_string())?;
    env.set_static_field("zombie/network/GameClient", client, JValue::Bool(0))
        .map_err(|e| e.to_string())?;
    let mut context = fresh(env)?;
    trigger(&mut context, "OnGameStart")?;
    lua(
        &mut context,
        "assert(not isolationRan); assert(LuaBuddy == nil, 'dedicated server must keep its native environment untouched')",
    )?;
    drop(context);
    env.set_static_field("zombie/network/GameServer", server, JValue::Bool(0))
        .map_err(|e| e.to_string())?;
    env.set_static_field("zombie/network/GameClient", client, JValue::Bool(1))
        .map_err(|e| e.to_string())?;
    println!("PASS dedicated-server loading isolation");
    Ok(())
}

fn dependency_errors(env: &mut JNIEnv<'_>, fixture: &Fixture) -> Result<()> {
    fixture.clear_profiles()?;
    for id in [
        "MissingRequirement",
        "MissingDependent",
        "OrderCycleA",
        "OrderCycleB",
        "DependencyUnrelated",
    ] {
        fixture.module(id, &format!("client/{id}.lua"), &format!("{id}Ran = true"))?;
    }
    fixture.write("mods/MissingRequirement/42/mod.info", "name=MissingRequirement\nid=MissingRequirement\nrequire=AbsentNativeDependency\nversionMin=42.0\n")?;
    fixture.profile("MissingRequirement", "")?;
    fixture.profile("MissingDependent", "requires=MissingRequirement\n")?;
    fixture.profile("OrderCycleA", "after=OrderCycleB\n")?;
    fixture.profile("OrderCycleB", "after=OrderCycleA\n")?;
    fixture.profile("DependencyUnrelated", "")?;
    let mut context = fresh(env)?;
    trigger(&mut context, "OnGameStart")?;
    lua(
        &mut context,
        "for _, id in ipairs({'MissingRequirement', 'MissingDependent', 'OrderCycleA', 'OrderCycleB'}) do assert(LuaBuddy.mods[id]:find('failed:', 1, true) == 1, id) end; assert(not MissingRequirementRan and not MissingDependentRan and not OrderCycleARan and not OrderCycleBRan); assert(LuaBuddy.mods.DependencyUnrelated == 'loaded' and DependencyUnrelatedRan)",
    )?;
    drop(context);
    fixture.clear_profiles()?;
    fixture.module(
        "InvalidConfiguration",
        "client/InvalidConfiguration.lua",
        "invalidConfigurationRuns = (invalidConfigurationRuns or 0) + 1",
    )?;
    fixture.profile("InvalidConfiguration", "typo=true\n")?;
    let mut context = fresh(env)?;
    trigger(&mut context, "OnGameStart")?;
    lua(&mut context, "assert(not invalidConfigurationRuns)")?;
    let diagnostic = report(fixture)?;
    let error = diagnostic["error"]
        .as_str()
        .ok_or("Invalid configuration had no diagnostic error")?;
    if !error.contains("typo") || !error.contains("InvalidConfiguration.properties") {
        return Err(format!(
            "Invalid profile error lacks exact option/file: {error}"
        ));
    }
    fixture.profile("InvalidConfiguration", "")?;
    let (ok, reason) = refresh(&mut context)?;
    if !ok {
        return Err(format!(
            "Corrected pre-mutation configuration could not load: {reason}"
        ));
    }
    lua(
        &mut context,
        "assert(LuaBuddy.mods.InvalidConfiguration == 'loaded' and invalidConfigurationRuns == 1)",
    )?;
    println!(
        "PASS native missing dependencies, order cycles, unrelated continuation and invalid-profile correction before mutation"
    );
    Ok(())
}

fn native_id(env: &mut JNIEnv<'_>, fixture: &Fixture, id: &str, active: bool) -> Result<()> {
    let filesystem = env
        .get_static_field(
            "zombie/ZomboidFileSystem",
            "instance",
            "Lzombie/ZomboidFileSystem;",
        )
        .and_then(|value| value.l())
        .map_err(|e| e.to_string())?;
    let name = env.new_string(id).map_err(|e| e.to_string())?;
    if active {
        let path = env
            .new_string(fixture.root.join("mods").join(id).to_string_lossy())
            .map_err(|e| e.to_string())?;
        env.call_method(
            &filesystem,
            "setModIdToDir",
            "(Ljava/lang/String;Ljava/lang/String;)V",
            &[JValue::Object(&name), JValue::Object(&path)],
        )
        .map_err(|e| e.to_string())?;
    }
    let list = env
        .call_method(filesystem, "getModIDs", "()Ljava/util/List;", &[])
        .and_then(|value| value.l())
        .map_err(|e| e.to_string())?;
    env.call_method(
        list,
        if active { "add" } else { "remove" },
        "(Ljava/lang/Object;)Z",
        &[JValue::Object(&name)],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn native_ordering(env: &mut JNIEnv<'_>, fixture: &Fixture) -> Result<()> {
    let mut failures = Vec::new();
    for relation in ["before", "profileAfter", "nativeAfter"] {
        for runtime in [false, true] {
            let suffix = format!("{relation}{runtime}");
            let native = format!("Native_{suffix}");
            let incoming = format!("Incoming_{suffix}");
            fixture.clear_profiles()?;
            fixture.module(
                &native,
                "client/Native.lua",
                "error('server-active native source must be skipped')",
            )?;
            if relation == "nativeAfter" {
                fixture.write(
                    &format!("mods/{native}/42/mod.info"),
                    &format!(
                        "name=Native\nid={native}\nversionMin=42.0\nloadModAfter={incoming}\n"
                    ),
                )?;
            }
            native_id(env, fixture, &native, true)?;
            let result = (|| -> Result<()> {
                let mut context = if runtime {
                    let mut context = fresh(env)?;
                    trigger(&mut context, "OnGameStart")?;
                    Some(context)
                } else {
                    None
                };
                fixture.module(&incoming, "client/Order.lua", "nativeOrderRan = true")?;
                fixture.write(
                    &format!("mods/{incoming}/42/media/ui/{suffix}.png"),
                    "must stay unpublished",
                )?;
                fixture.profile(
                    &incoming,
                    &format!(
                        "mount=true\n{}",
                        if relation == "before" {
                            format!("before={native}\n")
                        } else {
                            String::new()
                        }
                    ),
                )?;
                if relation == "profileAfter" {
                    fixture.profile(&native, &format!("after={incoming}\n"))?;
                }
                if let Some(context) = &mut context {
                    let (ok, reason) = refresh(context)?;
                    if ok {
                        return Err(format!(
                            "Native reciprocal {suffix} order was accepted: {reason}"
                        ));
                    }
                } else {
                    let mut current = fresh(env)?;
                    trigger(&mut current, "OnGameStart")?;
                    context = Some(current);
                }
                let context = context
                    .as_mut()
                    .ok_or("Missing native ordering environment")?;
                lua(
                    context,
                    &format!(
                        "assert(not nativeOrderRan); assert(LuaBuddy.mods['{incoming}']:find('order requires', 1, true), LuaBuddy.mods['{incoming}'])"
                    ),
                )?;
                let image = context.jni(|env| {
                    let filesystem = env
                        .get_static_field(
                            "zombie/ZomboidFileSystem",
                            "instance",
                            "Lzombie/ZomboidFileSystem;",
                        )?
                        .l()?;
                    let name = env.new_string(format!("media/ui/{suffix}.png"))?;
                    let path = env
                        .call_method(
                            filesystem,
                            "getString",
                            "(Ljava/lang/String;)Ljava/lang/String;",
                            &[JValue::Object(&name)],
                        )?
                        .l()?;
                    env.new_global_ref(path).map(|value| Value(Some(value)))
                })?;
                if context.text(&image)?
                    == fixture
                        .root
                        .join(format!("mods/{incoming}/42/media/ui/{suffix}.png"))
                        .to_string_lossy()
                {
                    return Err(format!(
                        "Rejected native order {suffix} published candidate assets"
                    ));
                }
                Ok(())
            })();
            native_id(env, fixture, &native, false)?;
            if let Err(error) = result {
                failures.push(format!("{suffix}: {error}"));
            }
        }
    }
    if !failures.is_empty() {
        return Err(failures.join("\n"));
    }
    println!(
        "PASS initial/runtime native ordering, reciprocal profile/native metadata and preflight-before-mount refusal"
    );
    Ok(())
}

fn set_property(env: &mut JNIEnv<'_>, key: &str, value: Option<&str>) -> Result<()> {
    let key = env.new_string(key).map_err(|e| e.to_string())?;
    match value {
        Some(value) => {
            let value = env.new_string(value).map_err(|e| e.to_string())?;
            env.call_static_method(
                "java/lang/System",
                "setProperty",
                "(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;",
                &[JValue::Object(&key), JValue::Object(&value)],
            )
            .map_err(|e| e.to_string())?;
        }
        None => {
            env.call_static_method(
                "java/lang/System",
                "clearProperty",
                "(Ljava/lang/String;)Ljava/lang/String;",
                &[JValue::Object(&key)],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn early_ui_timing(env: &mut JNIEnv<'_>, fixture: &Fixture) -> Result<()> {
    for (enabled, early_event) in [(false, true), (true, true), (true, false)] {
        fixture.clear_profiles()?;
        fixture.module(
            "Timing",
            "client/Timing.lua",
            "timingRuns = (timingRuns or 0) + 1",
        )?;
        fixture.profile("Timing", "")?;
        set_property(
            env,
            "luabuddy.earlyUI",
            Some(if enabled { "true" } else { "false" }),
        )?;
        let mut context = fresh(env)?;
        lua(
            &mut context,
            "Events.OnCreateUI.Add(function() earlySawMod = timingRuns == 1; if LuaBuddy then earlyRequest = LuaBuddy.refresh() end end)",
        )?;
        if early_event {
            trigger(&mut context, "OnCreateUI")?;
            lua(
                &mut context,
                if enabled {
                    "assert(earlySawMod and timingRuns == 1 and earlyRequest == false and not LuaBuddy.ready)"
                } else {
                    "assert(not earlySawMod and not timingRuns and LuaBuddy == nil)"
                },
            )?;
        }
        trigger(&mut context, "OnGameStart")?;
        lua(&mut context, "assert(timingRuns == 1 and LuaBuddy.ready)")?;
        if enabled && early_event {
            trigger(&mut context, "OnCreateUI")?;
            lua(&mut context, "assert(timingRuns == 1 and LuaBuddy.ready)")?;
        }
    }
    set_property(env, "luabuddy.earlyUI", None)?;
    println!(
        "PASS explicit early-UI timing, default native timing, game-start fallback and once-per-environment loading"
    );
    Ok(())
}

fn namespace_ownership(env: &mut JNIEnv<'_>, fixture: &Fixture) -> Result<()> {
    fixture.clear_profiles()?;
    fixture.module(
        "Namespace",
        "client/Namespace.lua",
        "namespaceRuns = (namespaceRuns or 0) + 1; return {identity = 42}",
    )?;
    fixture.profile("Namespace", "")?;
    let mut context = fresh(env)?;
    trigger(&mut context, "OnGameStart")?;
    let state = context.global("LuaBuddy")?;
    let refresh = context.get(&state, "refresh")?;
    let require = context.global("require")?;
    let name = context.string("Namespace")?;
    let object = context
        .call(&require, std::slice::from_ref(&name))?
        .remove(0);
    fixture.module(
        "NamespaceNew",
        "client/NamespaceNew.lua",
        "namespaceNewRan = true",
    )?;
    fixture.profile("NamespaceNew", "")?;
    let foreign = context.table()?;
    let environment = context.environment.clone();
    context.set(&environment, "LuaBuddy", &foreign)?;
    let result = context.call(&refresh, &[])?;
    if result.is_empty() || context.truth(&result[0])? {
        return Err("A captured operation accepted a replaced runtime namespace".into());
    }
    lua(
        &mut context,
        "assert(not namespaceNewRan and namespaceRuns == 1); assert(LuaBuddy.refresh == nil)",
    )?;
    let cached = context.call(&require, &[name])?.remove(0);
    if !context.same(&object, &cached)? {
        return Err("A captured resolver lost identity after namespace replacement".into());
    }
    context.set(&environment, "LuaBuddy", &state)?;
    let result = context.call(&refresh, &[])?;
    if result.is_empty() || !context.truth(&result[0])? {
        return Err("Restoring the owned namespace did not restore the original session".into());
    }
    lua(
        &mut context,
        "assert(namespaceNewRan and namespaceRuns == 1)",
    )?;
    fixture.clear_profiles()?;
    fixture.module(
        "NamespaceConflict",
        "client/NamespaceConflict.lua",
        "namespaceConflictRan = true",
    )?;
    fixture.profile("NamespaceConflict", "")?;
    let mut context = fresh(env)?;
    let foreign = context.table()?;
    let environment = context.environment.clone();
    context.set(&environment, "LuaBuddy", &foreign)?;
    trigger(&mut context, "OnGameStart")?;
    let current = context.global("LuaBuddy")?;
    if !context.same(&current, &foreign)? {
        return Err("Initialization replaced a foreign namespace".into());
    }
    lua(
        &mut context,
        "assert(not namespaceConflictRan and LuaBuddy.refresh == nil)",
    )?;
    println!(
        "PASS namespace ownership refusal, preserved captured resolver, and safe initialization conflict"
    );
    Ok(())
}

fn nested_startup_readiness(env: &mut JNIEnv<'_>, fixture: &Fixture) -> Result<()> {
    fixture.clear_profiles()?;
    fixture.module("NestedStart", "client/NestedStart.lua", "nestedModuleRuns = (nestedModuleRuns or 0) + 1; Events.OnGameStart.Add(function() assert(not LuaBuddy.ready, 'startup became ready inside dispatch'); if not nestedStarted then nestedStarted = true; triggerEvent('OnGameStart'); nestedAfter = LuaBuddy.ready end end)")?;
    fixture.profile("NestedStart", "")?;
    let mut context = fresh(env)?;
    trigger(&mut context, "OnGameStart")?;
    lua(
        &mut context,
        "assert(nestedModuleRuns == 1 and nativeStarts == 2 and nestedAfter == false and LuaBuddy.ready == true)",
    )?;
    println!("PASS nested startup remains unavailable until the outer actual dispatcher completes");
    Ok(())
}

fn startup_eligibility_transitions(env: &mut JNIEnv<'_>, fixture: &Fixture) -> Result<()> {
    fixture.clear_profiles()?;
    fixture.module(
        "StartupScope",
        "client/StartupScope.lua",
        "startupScopeRuns = (startupScopeRuns or 0) + 1",
    )?;
    fixture.write(
        "mods/StartupScope/42/media/FixtureTiles.tiles",
        "native tile definition fixture",
    )?;
    fixture.profile("StartupScope", "live=auto\n")?;
    let mut context = fresh(env)?;
    trigger(&mut context, "OnGameStart")?;
    lua(
        &mut context,
        "assert(not startupScopeRuns and LuaBuddy.mods.StartupScope:find('restart-required:', 1, true) == 1)",
    )?;
    let before = report(fixture)?;
    if before["mods"]["StartupScope"]["status"] != "restart-required"
        || before["summary"]["restartRequired"] != 1
    {
        return Err(
            "Native startup refusal was not represented separately from script failure".into(),
        );
    }
    fixture.profile("StartupScope", "live=client-shared\n")?;
    if !refresh(&mut context)?.0 {
        return Err(
            "Explicit client/shared opt-in could not activate a previously refused selection"
                .into(),
        );
    }
    lua(
        &mut context,
        "assert(startupScopeRuns == 1 and LuaBuddy.mods.StartupScope == 'loaded')",
    )?;
    let after = report(fixture)?;
    if after["mods"]["StartupScope"].get("error").is_some()
        || after["summary"]["restartRequired"] != 0
        || !after["mods"]["StartupScope"]["activationLimit"]
            .as_str()
            .is_some_and(|text| text.contains("tile definitions"))
    {
        return Err("Loaded client/shared diagnostics retained a stale refusal or omitted the unprovided native component".into());
    }
    fixture.module(
        "StartupOnly",
        "client/StartupOnly.lua",
        "startupOnlyRan = true",
    )?;
    fixture.profile("StartupOnly", "live=startup\n")?;
    if refresh(&mut context)?.0 {
        return Err("A native-startup-only selection reported live activation success".into());
    }
    lua(
        &mut context,
        "assert(not startupOnlyRan and startupScopeRuns == 1)",
    )?;
    fixture.profile("StartupOnly", "live=startup\nenabled=false\n")?;
    if !refresh(&mut context)?.0 {
        return Err("Disabling an unexecuted startup-only selection retained the refusal".into());
    }
    let after = report(fixture)?;
    if after["mods"]["StartupOnly"]["status"] != "disabled"
        || after["mods"]["StartupOnly"]["stage"] != "disabled"
        || after["mods"]["StartupOnly"].get("error").is_some()
    {
        return Err("Disabled selection diagnostics retained stale activation state".into());
    }
    println!(
        "PASS native startup refusal before mutation, explicit scoped activation, and accurate retry/disable diagnostics"
    );
    Ok(())
}

fn renamed_disabled_selection(env: &mut JNIEnv<'_>, fixture: &Fixture) -> Result<()> {
    fixture.clear_profiles()?;
    fixture.module(
        "Renamed",
        "client/Renamed.lua",
        "renamedRuns = (renamedRuns or 0) + 1",
    )?;
    fixture.write(
        "config/a.properties",
        "id=Renamed\npath=../mods/Renamed\nenabled=false\n",
    )?;
    let mut context = fresh(env)?;
    trigger(&mut context, "OnGameStart")?;
    lua(
        &mut context,
        "assert(not renamedRuns and LuaBuddy.mods.Renamed == 'disabled')",
    )?;
    std::fs::rename(
        fixture.root.join("config/a.properties"),
        fixture.root.join("config/b.properties"),
    )
    .map_err(|error| error.to_string())?;
    fixture.write(
        "config/b.properties",
        "id=Renamed\npath=../mods/Renamed\nenabled=true\n",
    )?;
    if !refresh(&mut context)?.0 {
        return Err("Renamed disabled selection did not load".into());
    }
    lua(
        &mut context,
        "assert(renamedRuns == 1 and LuaBuddy.mods.Renamed == 'loaded')",
    )?;
    let report = report(fixture)?;
    let expected = fixture
        .root
        .join("config/b.properties")
        .to_string_lossy()
        .into_owned();
    if report["mods"]["Renamed"]["profile"].as_str() != Some(expected.as_str()) {
        return Err("Loaded-generation diagnostics still name the old profile file".into());
    }
    println!(
        "PASS renamed previously disabled selection executes once and reports the actual configuration file"
    );
    Ok(())
}

fn profile_scan_order(env: &mut JNIEnv<'_>, fixture: &Fixture) -> Result<()> {
    fixture.clear_profiles()?;
    for (file, id, extra) in [
        ("a", "Zulu", ""),
        ("b", "Beta", "requires=Zulu\n"),
        ("c", "Alpha", ""),
    ] {
        fixture.module(
            id,
            "shared/SharedWins.lua",
            &format!(
                "scanOrder = scanOrder or {{}}; table.insert(scanOrder, '{id}'); return '{id}'"
            ),
        )?;
        fixture.write(
            &format!("config/{file}.properties"),
            &format!("id={id}\npath=../mods/{id}\n{extra}"),
        )?;
    }
    let mut context = fresh(env)?;
    trigger(&mut context, "OnGameStart")?;
    lua(
        &mut context,
        "assert(#scanOrder == 3 and scanOrder[1] == 'Zulu' and scanOrder[2] == 'Beta' and scanOrder[3] == 'Alpha', 'profile-file scan order changed'); assert(require('SharedWins') == 'Alpha', 'winning source provider changed'); assert(LuaBuddy.mods.Zulu == 'loaded' and LuaBuddy.mods.Beta == 'loaded' and LuaBuddy.mods.Alpha == 'loaded')",
    )?;
    println!(
        "PASS original profile-file scan order, same-pass dependencies and winning module-provider precedence"
    );
    Ok(())
}
