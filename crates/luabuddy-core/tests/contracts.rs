use luabuddy_core::lifecycle::{
    Candidate, CleanupDecision, CleanupOutcome, GenerationState, Operation, RequestContext, Session,
};
use luabuddy_core::profile::parse_properties;
use luabuddy_core::source::{
    Generation, Glob, ModuleKey, ModuleRegistry, Owner, Require, Source, scan_sources,
};
use luabuddy_core::{
    ModId, NativeMetadata, Profile, ReplayEvent, ResolvedProfile, SkipReason, read_profiles,
    resolve,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

fn id(value: &str) -> ModId {
    ModId::new(value).unwrap()
}
fn profile(value: &str, settings: &str) -> Profile {
    Profile::parse(
        format!("{value}.properties"),
        &format!("id={value}\n{settings}"),
    )
    .unwrap()
}
fn metadata(value: &str) -> NativeMetadata {
    NativeMetadata {
        id: Some(id(value)),
        available: true,
        ..Default::default()
    }
}
fn source(name: &str) -> Source {
    Source {
        name: name.to_owned(),
        filename: PathBuf::from(format!("mod/{name}")),
        text: String::new(),
    }
}
fn resolved(value: &str, settings: &str) -> ResolvedProfile {
    let profile = profile(value, settings);
    let native = BTreeMap::from([(id(value), metadata(value))]);
    let resolution = resolve(&[profile], &native, &BTreeSet::new());
    resolution.ordered.into_iter().next().unwrap()
}
fn candidate(value: &str, settings: &str, cleanup: bool) -> Candidate<()> {
    Candidate {
        profile: resolved(value, settings),
        sources: vec![source(&format!("shared/{value}.lua"))],
        compiled: (),
        cleanup_supported: cleanup,
    }
}
fn context() -> RequestContext {
    RequestContext {
        environment: 7,
        multiplayer_client: true,
        pending_options: false,
    }
}
fn loaded_session(cleanup: bool) -> Session {
    let mut session = Session::new(7, BTreeSet::new());
    let token = session.begin(context(), Operation::Startup).unwrap();
    let prepared = session
        .prepare(&token, vec![candidate("A", "", cleanup)])
        .unwrap()
        .pop()
        .unwrap();
    let CleanupDecision::Activate(permit) = session
        .after_cleanup(prepared, CleanupOutcome::Accepted)
        .unwrap()
    else {
        panic!("new candidate does not require cleanup");
    };
    session.finish_activation(permit, Ok(())).unwrap();
    session.end(&token).unwrap();
    session.set_ready(true).unwrap();
    session
}

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "luabuddy-core-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
    fn write(&self, name: &str, text: &str) {
        let file = self.0.join(name);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn java_properties_escapes_continuation_unicode_and_duplicate_keys() {
    let values = parse_properties(" # ignored\\\r\nid : Old\r\nid=New\\\r\n   Name\r\na\\:b\\=c = value\\twith\\ space\nemoji=\\uD83D\\uDE00\n").unwrap();
    assert_eq!(values["id"], "NewName");
    assert_eq!(values["a:b=c"], "value\twith space");
    assert_eq!(values["emoji"], "😀");
    for invalid in ["id=\\u12", "id=\\uZZZZ", "id=\\uD800", "id=\\uDC00"] {
        assert!(parse_properties(invalid).is_err());
    }
}

#[test]
fn profile_validation_is_strict_and_rust_hooks_preserve_replay_settings() {
    for settings in [
        "enabled=yes",
        "mount=1",
        "replay=OnTick",
        "workshop=0",
        "workshop=1234567890123456789",
        "unknown=true",
        "excludeWhen.=x",
        "hook=bad/path",
    ] {
        assert!(
            Profile::parse("example.properties", &format!("id=A\n{settings}")).is_err(),
            "{settings}"
        );
    }
    for settings in ["plugin=", "plugin=custom.lua", "hook=RustHook\nplugin="] {
        let error = Profile::parse("example.properties", &format!("id=A\n{settings}")).unwrap_err();
        assert_eq!(error.stage, "profile");
        assert_eq!(error.path, Some(PathBuf::from("example.properties")));
        assert!(error.message.contains("Unknown profile option plugin"));
    }
    let parsed = profile("A", "hook=RustHook\nreplay=OnGameBoot;OnGameStart;OnLoad");
    assert_eq!(parsed.hook.as_deref(), Some("RustHook"));
    assert_eq!(
        parsed.replay,
        BTreeSet::from([
            ReplayEvent::OnGameBoot,
            ReplayEvent::OnGameStart,
            ReplayEvent::OnLoad,
        ])
    );
}

#[test]
fn empty_selection_and_duplicate_profile_identity() {
    let directory = Directory::new();
    assert!(read_profiles(&directory.0).unwrap().is_empty());
    directory.write("b.properties", "id=B");
    directory.write("a.properties", "id=A");
    assert_eq!(
        read_profiles(&directory.0)
            .unwrap()
            .iter()
            .map(|p| p.id.as_str())
            .collect::<Vec<_>>(),
        ["A", "B"]
    );
    directory.write("duplicate.properties", "id=A");
    assert!(
        read_profiles(&directory.0)
            .unwrap_err()
            .to_string()
            .contains("Duplicate profile")
    );
}

#[test]
fn native_and_profile_dependency_order_merge_without_optional_requirements() {
    let profiles = vec![
        profile("C", "before=D"),
        profile("D", ""),
        profile("B", "requires=A\nafter=Absent"),
        profile("A", ""),
    ];
    let mut native: BTreeMap<_, _> = ["A", "B", "C", "D"]
        .into_iter()
        .map(|value| (id(value), metadata(value)))
        .collect();
    native.get_mut(&id("C")).unwrap().requires.push(id("B"));
    let resolution = resolve(&profiles, &native, &BTreeSet::new());
    assert!(resolution.failures.is_empty());
    assert_eq!(
        resolution
            .ordered
            .iter()
            .map(|p| p.profile.id.as_str())
            .collect::<Vec<_>>(),
        ["A", "B", "C", "D"]
    );
}

#[test]
fn dependency_scan_retains_profile_file_order_and_same_pass_readiness() {
    let directory = Directory::new();
    directory.write("a.properties", "id=Zulu\n");
    directory.write("b.properties", "id=Beta\nrequires=Zulu\n");
    directory.write("c.properties", "id=Alpha\n");
    let profiles = read_profiles(&directory.0).unwrap();
    let native = ["Alpha", "Beta", "Zulu"]
        .into_iter()
        .map(|value| (id(value), metadata(value)))
        .collect();
    let resolution = resolve(&profiles, &native, &BTreeSet::new());
    assert!(resolution.failures.is_empty());
    assert_eq!(
        resolution
            .ordered
            .iter()
            .map(|profile| profile.profile.id.as_str())
            .collect::<Vec<_>>(),
        ["Zulu", "Beta", "Alpha"]
    );
}

#[test]
fn cycles_missing_dependencies_and_identity_failure_do_not_block_unrelated_mods() {
    let profiles = vec![
        profile("A", "requires=B"),
        profile("B", "requires=A"),
        profile("C", "requires=Absent"),
        profile("D", ""),
        profile("E", ""),
    ];
    let mut native: BTreeMap<_, _> = ["A", "B", "C", "D", "E"]
        .into_iter()
        .map(|value| (id(value), metadata(value)))
        .collect();
    native.get_mut(&id("E")).unwrap().id = Some(id("Wrong"));
    let resolution = resolve(&profiles, &native, &BTreeSet::new());
    assert_eq!(resolution.ordered.len(), 1);
    assert_eq!(resolution.ordered[0].profile.id, id("D"));
    assert_eq!(resolution.failures.len(), 4);
}

#[test]
fn disabled_and_native_loaded_profiles_skip_and_native_reciprocal_order_is_retained() {
    let profiles = vec![
        profile("A", ""),
        profile("B", "after=A"),
        profile("Disabled", "enabled=false"),
    ];
    let native = BTreeMap::from([(id("A"), metadata("A")), (id("B"), metadata("B"))]);
    let result = resolve(&profiles, &native, &BTreeSet::from([id("B")]));
    assert_eq!(result.skipped[&id("B")], SkipReason::NativeLoaded);
    assert_eq!(result.skipped[&id("Disabled")], SkipReason::Disabled);
    assert!(
        result.failures[&id("A")]
            .message
            .contains("already loaded native")
    );
}

#[test]
fn source_scan_uses_native_roots_and_version_overlay_with_conditional_exclusions() {
    let directory = Directory::new();
    directory.write("common/media/lua/shared/Thing.lua", "common");
    directory.write("version/media/lua/shared/thing.LUA", "version");
    directory.write("common/media/lua/client/Thing.lua", "client");
    directory.write("common/media/lua/client/Excluded.lua", "excluded");
    directory.write("common/media/lua/server/Server.lua", "server");
    directory.write(
        "common/media/lua/shared/Translate/EN/Example.lua",
        "translation",
    );
    let roots = [directory.0.join("common"), directory.0.join("version")];
    let profile = profile("A", "excludeWhen.Other=client/Excluded.lua");
    let sources = scan_sources(&profile, &roots, &BTreeSet::from([id("Other")])).unwrap();
    assert_eq!(sources.len(), 2);
    assert_eq!(
        sources
            .iter()
            .find(|s| s.name.to_lowercase() == "shared/thing.lua")
            .unwrap()
            .text,
        "version"
    );
    assert!(sources.iter().all(|s| s.filename.is_absolute()));
}

#[test]
fn glob_contract_supports_native_profile_patterns() {
    for (pattern, yes, no) in [
        ("client/*.lua", "client/Test.lua", "client/sub/Test.lua"),
        (
            "**/Test.[lL][uU][aA]",
            "client/sub/Test.LUA",
            "client/Test.txt",
        ),
        ("client/{One,Two}.lua", "client/Two.lua", "client/Three.lua"),
        ("client/?[!0-9].lua", "client/ab.lua", "client/a2.lua"),
    ] {
        let pattern = Glob::parse(pattern).unwrap();
        assert!(pattern.matches(yes));
        assert!(!pattern.matches(no));
    }
    for invalid in ["[", "{a,b", "a}", "[z-a]"] {
        assert!(Glob::parse(invalid).is_err());
    }
}

#[test]
fn require_preserves_false_nil_identity_native_fallback_and_one_execution() {
    let owner = Owner {
        id: id("A"),
        generation: Generation(1),
    };
    let mut registry = ModuleRegistry::<bool>::default();
    registry
        .register(
            owner.clone(),
            vec![source("shared/False.lua"), source("shared/Nil.lua")],
        )
        .unwrap();
    for (name, value) in [("False", Some(false)), ("Nil", None)] {
        let Require::Execute(key) = registry.require(name).unwrap() else {
            panic!("first require executes");
        };
        registry.begin(&key).unwrap();
        assert!(
            registry
                .require(name)
                .unwrap_err()
                .message
                .contains("Circular")
        );
        registry.finish(&key, Ok(value)).unwrap();
        assert!(registry.begin(&key).is_err());
    }
    assert!(matches!(
        registry.require("FALSE.LUA").unwrap(),
        Require::Cached(Some(false))
    ));
    assert!(matches!(
        registry.require("Nil").unwrap(),
        Require::Cached(None)
    ));
    assert!(matches!(
        registry.require("native/module").unwrap(),
        Require::Delegate
    ));
    let mut objects = ModuleRegistry::<Arc<String>>::default();
    objects
        .register(owner, vec![source("shared/Object.lua")])
        .unwrap();
    let key = ModuleKey::new("shared/Object").unwrap();
    let object = Arc::new("identity".to_owned());
    objects.begin(&key).unwrap();
    objects.finish(&key, Ok(Some(object.clone()))).unwrap();
    let Require::Cached(Some(cached)) = objects.require("Object").unwrap() else {
        panic!("cached object");
    };
    assert!(Arc::ptr_eq(cached, &object));
}

#[test]
fn shared_import_alias_wins_while_shared_and_client_sources_stay_distinct() {
    let mut registry = ModuleRegistry::<bool>::default();
    registry
        .register(
            Owner {
                id: id("A"),
                generation: Generation(1),
            },
            vec![source("shared/X.lua"), source("client/X.lua")],
        )
        .unwrap();
    let Require::Execute(shared) = registry.require("x").unwrap() else {
        panic!();
    };
    let Require::Execute(client) = registry.require("CLIENT\\X.LUA").unwrap() else {
        panic!();
    };
    assert_ne!(shared, client);
    assert_eq!(shared.as_str(), "shared/x");
    registry.begin(&shared).unwrap();
    registry
        .finish(&shared, Err("original error".to_owned()))
        .unwrap();
    assert_eq!(registry.require("X").unwrap_err().message, "original error");
    assert!(matches!(
        registry.require("client/X").unwrap(),
        Require::Execute(_)
    ));
}

#[test]
fn cleanup_refusal_retains_original_generation_and_original_activation_context() {
    let mut session = loaded_session(true);
    let original = session.running()[&id("A")].owner.clone();
    let token = session
        .begin(context(), Operation::Reload(id("A")))
        .unwrap();
    let prepared = session
        .prepare(&token, vec![candidate("A", "", true)])
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(prepared.replaces(), Some(&original));
    assert!(matches!(
        session
            .after_cleanup(prepared, CleanupOutcome::Refused("busy".into()))
            .unwrap(),
        CleanupDecision::Retained(_)
    ));
    assert_eq!(session.running()[&id("A")].owner, original);
    assert_eq!(session.running()[&id("A")].state, GenerationState::Loaded);
    assert_eq!(session.running()[&id("A")].activation, Operation::Startup);
    session.end(&token).unwrap();
}

#[test]
fn uncertain_cleanup_and_failed_activation_quarantine_without_claiming_rollback() {
    for accepted in [false, true] {
        let mut session = loaded_session(true);
        let token = session
            .begin(context(), Operation::Reload(id("A")))
            .unwrap();
        let prepared = session
            .prepare(&token, vec![candidate("A", "", true)])
            .unwrap()
            .pop()
            .unwrap();
        if accepted {
            let CleanupDecision::Activate(permit) = session
                .after_cleanup(prepared, CleanupOutcome::Accepted)
                .unwrap()
            else {
                panic!();
            };
            assert!(matches!(
                session.running()[&id("A")].state,
                GenerationState::Quarantined(_)
            ));
            session
                .finish_activation(permit, Err("replacement failed".into()))
                .unwrap();
        } else {
            assert!(matches!(
                session
                    .after_cleanup(
                        prepared,
                        CleanupOutcome::Uncertain("partial cleanup".into())
                    )
                    .unwrap(),
                CleanupDecision::Quarantined(_)
            ));
        }
        assert!(matches!(
            session.running()[&id("A")].state,
            GenerationState::Quarantined(_)
        ));
        session.end(&token).unwrap();
        let token = session
            .begin(context(), Operation::Reload(id("A")))
            .unwrap();
        assert!(
            session
                .prepare(&token, vec![candidate("A", "", true)])
                .is_err()
        );
        session.end(&token).unwrap();
    }
}

#[test]
fn refresh_noop_and_preflight_conflicts_do_not_replace_existing_generation() {
    let mut session = loaded_session(false);
    let original = session.running()[&id("A")].owner.clone();
    let token = session.begin(context(), Operation::Refresh).unwrap();
    assert!(
        session
            .prepare(&token, vec![candidate("A", "", false)])
            .unwrap()
            .is_empty()
    );
    let mut conflicting = candidate("B", "", false);
    conflicting.sources = vec![source("shared/A.lua")];
    assert!(
        session
            .prepare(&token, vec![conflicting])
            .unwrap_err()
            .message
            .contains("conflicts")
    );
    assert_eq!(session.running()[&id("A")].owner, original);
    assert_eq!(session.running()[&id("A")].state, GenerationState::Loaded);
    session.end(&token).unwrap();
    let token = session
        .begin(context(), Operation::Reload(id("A")))
        .unwrap();
    assert!(
        session
            .prepare(&token, vec![candidate("A", "", true)])
            .unwrap_err()
            .message
            .contains("no Rust cleanup hook")
    );
    session.end(&token).unwrap();
}

#[test]
fn runtime_order_shared_source_and_active_dependents_require_reconnect() {
    let mut session = loaded_session(true);
    let token = session.begin(context(), Operation::Refresh).unwrap();
    assert!(
        session
            .prepare(&token, vec![candidate("B", "before=A", false)])
            .unwrap_err()
            .message
            .contains("already loaded")
    );
    let mut dependent = candidate("B", "", false);
    dependent.profile.requires.insert(id("A"));
    let prepared = session
        .prepare(&token, vec![dependent])
        .unwrap()
        .pop()
        .unwrap();
    let CleanupDecision::Activate(permit) = session
        .after_cleanup(prepared, CleanupOutcome::Accepted)
        .unwrap()
    else {
        panic!();
    };
    session.finish_activation(permit, Ok(())).unwrap();
    session.end(&token).unwrap();
    let token = session
        .begin(context(), Operation::Reload(id("A")))
        .unwrap();
    assert!(
        session
            .prepare(&token, vec![candidate("A", "", true)])
            .unwrap_err()
            .message
            .contains("depends")
    );
    session.end(&token).unwrap();
}

#[test]
fn lifecycle_rejects_pending_edits_retired_environment_reentrancy_and_other_thread() {
    let mut session = loaded_session(true);
    let mut pending = context();
    pending.pending_options = true;
    assert!(session.begin(pending, Operation::Refresh).is_err());
    let mut wrong = context();
    wrong.environment = 8;
    assert!(session.begin(wrong, Operation::Refresh).is_err());
    let token = session.begin(context(), Operation::Refresh).unwrap();
    assert!(session.begin(context(), Operation::Refresh).is_err());
    session.end(&token).unwrap();
    let mut session = std::thread::spawn(move || {
        assert!(session.begin(context(), Operation::Refresh).is_err());
        session
    })
    .join()
    .unwrap();
    session.retire().unwrap();
    assert!(session.begin(context(), Operation::Refresh).is_err());
}

#[test]
fn selection_edits_preserve_properties_and_existing_sources() {
    use luabuddy_core::selection::{self, Lifetime};
    let root = Directory::new();
    let source = root.0.join("source #= ü");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("original.txt"), "original source").unwrap();
    let file = root.0.join("kept.properties");
    fs::write(
        &file,
        "id=Kept\nenabled=false\nmount=true\nexclude=client/one.lua;\\\n client/two.lua\\",
    )
    .unwrap();
    let before = read_profiles(&root.0).unwrap().remove(0);
    let selected =
        selection::select(&root.0, &id("Kept"), None, Lifetime::Persistent, None).unwrap();
    let mut expected = before.clone();
    expected.enabled = true;
    assert_eq!(selected, expected);
    assert!(selection::deselect(&root.0, &id("Kept"), Lifetime::Persistent).unwrap());
    assert_eq!(read_profiles(&root.0).unwrap(), vec![before]);
    assert!(selection::select(&root.0, &id("kept"), None, Lifetime::Persistent, None).is_err());
    let template = "id=Templated\nmount=true\nexclude=client/skip.lua;\\\n client/other.lua\\";
    let templated = selection::select(
        &root.0,
        &id("Templated"),
        Some(&source),
        Lifetime::Persistent,
        Some(template),
    )
    .unwrap();
    assert!(templated.enabled && templated.mount);
    assert_eq!(templated.exclude, ["client/skip.lua", "client/other.lua"]);
    assert_eq!(templated.path, Some(source.canonicalize().unwrap()));
    assert!(read_profiles(&root.0).unwrap().contains(&templated));
    fs::remove_file(&templated.file).unwrap();
    let queued = selection::select(
        &root.0,
        &id("Temporary ü"),
        Some(&source),
        Lifetime::Once,
        None,
    )
    .unwrap();
    assert_eq!(queued.path, Some(source.canonicalize().unwrap()));
    assert_eq!(selection::pending(&root.0).unwrap(), vec![queued]);
    assert!(selection::deselect(&root.0, &id("Temporary ü"), Lifetime::Once).unwrap());
    assert!(!selection::deselect(&root.0, &id("Temporary ü"), Lifetime::Once).unwrap());
    assert_eq!(
        fs::read_to_string(source.join("original.txt")).unwrap(),
        "original source"
    );
}

#[test]
fn one_launch_claim_has_one_consumer_and_preserves_normal_selection() {
    use luabuddy_core::selection::{self, Lifetime};
    let root = Directory::new();
    let persistent = root.0.join("Kept.properties");
    fs::write(&persistent, "id=Kept\nenabled=false\n").unwrap();
    let before = fs::read(&persistent).unwrap();
    assert!(selection::select(&root.0, &id("Kept"), None, Lifetime::Once, None).is_err());
    selection::select(&root.0, &id("Temporary"), None, Lifetime::Once, None).unwrap();
    assert!(
        selection::select(&root.0, &id("Temporary"), None, Lifetime::Persistent, None).is_err()
    );
    let barrier = std::sync::Barrier::new(2);
    let (a, b) = std::thread::scope(|scope| {
        let a = scope.spawn(|| {
            barrier.wait();
            selection::claim(&root.0).unwrap()
        });
        let b = scope.spawn(|| {
            barrier.wait();
            selection::claim(&root.0).unwrap()
        });
        (a.join().unwrap(), b.join().unwrap())
    });
    assert_eq!(a.len() + b.len(), 1);
    assert!(selection::pending(&root.0).unwrap().is_empty());
    assert!(selection::claim(&root.0).unwrap().is_empty());
    assert_eq!(fs::read(&persistent).unwrap(), before);
}

#[test]
fn one_launch_claim_contention_preserves_the_queue_until_the_owner_releases() {
    use luabuddy_core::selection::{self, Lifetime};
    let root = Directory::new();
    selection::select(&root.0, &id("Temporary"), None, Lifetime::Once, None).unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.0.join(".launch-once/claim.lock"))
        .unwrap();
    lock.lock().unwrap();
    assert!(selection::claim(&root.0).unwrap().is_empty());
    assert_eq!(selection::pending(&root.0).unwrap().len(), 1);
    drop(lock);
    let profiles = selection::claim(&root.0).unwrap();
    assert_eq!(profiles.len(), 1);
    assert_eq!(profiles[0].id, id("Temporary"));
    assert!(selection::pending(&root.0).unwrap().is_empty());
}

#[test]
fn one_launch_keeps_a_queue_that_conflicts_with_a_later_persistent_selection() {
    use luabuddy_core::selection::{self, Lifetime};
    let root = Directory::new();
    let source = root.0.join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("original.txt"), "keep this source").unwrap();
    selection::select(
        &root.0,
        &id("Temporary"),
        Some(&source),
        Lifetime::Once,
        None,
    )
    .unwrap();
    let persistent = root.0.join("user.properties");
    let settings = b"id=Temporary\nenabled=false\nmount=true\n";
    fs::write(&persistent, settings).unwrap();
    selection::select(&root.0, &id("Other"), None, Lifetime::Once, None).unwrap();
    let claimed = selection::claim(&root.0).unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].id, id("Other"));
    let skipped = selection::skipped(&root.0).unwrap();
    assert_eq!(skipped.len(), 1);
    assert!(
        skipped[0]
            .message
            .contains("conflicts with a persistent profile")
    );
    assert_eq!(fs::read(&persistent).unwrap(), settings);
    assert_eq!(
        fs::read_to_string(source.join("original.txt")).unwrap(),
        "keep this source"
    );
    // A conflicting entry is reported as skipped only, yet can still be cancelled.
    assert!(selection::pending(&root.0).unwrap().is_empty());
    assert!(selection::deselect(&root.0, &id("Temporary"), Lifetime::Once).unwrap());
    assert!(selection::claim(&root.0).unwrap().is_empty());
}

#[test]
fn one_launch_claims_valid_registrations_around_foreign_and_malformed_entries() {
    use luabuddy_core::selection::{self, Lifetime};
    let root = Directory::new();
    let persistent = root.0.join("user.properties");
    let settings = b"id=Kept\nenabled=true\n";
    fs::write(&persistent, settings).unwrap();
    selection::select(&root.0, &id("Good"), None, Lifetime::Once, None).unwrap();
    let pending = root.0.join(".launch-once/pending");
    let broken = pending.join("Broken.properties");
    fs::write(&broken, "id=Broken\nunknown=true\n").unwrap();
    fs::write(pending.join(".directory"), "[Desktop Entry]\n").unwrap();

    let claimed = selection::claim(&root.0).unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].id, id("Good"));
    let skipped = selection::skipped(&root.0).unwrap();
    assert_eq!(skipped.len(), 1);
    assert_eq!(
        skipped[0].path.as_deref().unwrap().file_name().unwrap(),
        "Broken.properties"
    );
    assert_eq!(
        fs::read_to_string(&broken).unwrap(),
        "id=Broken\nunknown=true\n"
    );
    assert!(pending.join(".directory").is_file());
    assert_eq!(fs::read(&persistent).unwrap(), settings);
    assert!(selection::claim(&root.0).unwrap().is_empty());
    let store: Vec<_> = fs::read_dir(root.0.join(".launch-once"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert!(
        store
            .iter()
            .all(|name| name == "pending" || name == "claim.lock"),
        "{store:?}"
    );
}

#[test]
fn one_launch_keeps_the_queue_when_a_persistent_profile_is_unreadable() {
    use luabuddy_core::selection::{self, Lifetime};
    let root = Directory::new();
    selection::select(&root.0, &id("Temporary"), None, Lifetime::Once, None).unwrap();
    let broken = root.0.join("Broken.properties");
    fs::write(&broken, "id=Broken\nunknown=true\n").unwrap();
    assert!(selection::claim(&root.0).is_err());
    fs::remove_file(&broken).unwrap();
    assert_eq!(selection::claim(&root.0).unwrap().len(), 1);
    assert!(!root.0.join(".launch-once/pending").exists());
}

#[cfg(unix)]
#[test]
fn selections_follow_a_linked_parent_but_not_a_linked_store() {
    use luabuddy_core::selection::{self, Lifetime};
    use std::os::unix::fs::symlink;
    let real = Directory::new();
    let links = Directory::new();
    fs::create_dir(real.0.join("profiles")).unwrap();
    let parent = links.0.join("home");
    symlink(&real.0, &parent).unwrap();
    let profiles = parent.join("profiles");
    selection::select(&profiles, &id("Kept"), None, Lifetime::Persistent, None).unwrap();
    selection::select(&profiles, &id("Temporary"), None, Lifetime::Once, None).unwrap();
    assert!(real.0.join("profiles/Kept.properties").is_file());
    assert_eq!(selection::claim(&profiles).unwrap().len(), 1);

    let outside = Directory::new();
    fs::remove_dir_all(real.0.join("profiles/.launch-once")).unwrap();
    symlink(&outside.0, real.0.join("profiles/.launch-once")).unwrap();
    assert!(selection::select(&profiles, &id("Temporary"), None, Lifetime::Once, None).is_err());
    assert_eq!(fs::read_dir(&outside.0).unwrap().count(), 0);

    let missing = links.0.join("typo");
    assert!(selection::select(&missing, &id("Kept"), None, Lifetime::Persistent, None).is_err());
    assert!(selection::deselect(&missing, &id("Kept"), Lifetime::Persistent).is_err());
    assert!(!missing.exists());
}

#[cfg(unix)]
#[test]
fn one_launch_refuses_symlink_registrations_without_following_them() {
    use luabuddy_core::selection;
    use std::os::unix::fs::symlink;
    let root = Directory::new();
    let source = Directory::new();
    let original = source.0.join("Original.properties");
    fs::write(&original, "id=Original\n").unwrap();
    let pending = root.0.join(".launch-once/pending");
    fs::create_dir_all(&pending).unwrap();
    symlink(&original, pending.join("Original.properties")).unwrap();
    assert!(selection::claim(&root.0).unwrap().is_empty());
    assert_eq!(selection::skipped(&root.0).unwrap().len(), 1);
    assert_eq!(fs::read_to_string(&original).unwrap(), "id=Original\n");
    assert!(pending.join("Original.properties").is_symlink());
}

#[test]
fn toggling_a_profile_keeps_its_other_lines_and_does_not_grow() {
    use luabuddy_core::profile::with_enabled;
    let original = "# chosen by hand\nid=Example\nreplay=OnGameStart\n";
    let off = with_enabled(original, false);
    assert_eq!(off, format!("{original}enabled=false\n"));
    let on = with_enabled(&off, true);
    assert_eq!(on, original);
    let mut text = original.to_owned();
    for _ in 0..20 {
        text = with_enabled(&with_enabled(&text, false), true);
    }
    assert_eq!(text, original);
    // A continued line that merely mentions the key is not removed.
    let continued = "id=Example\nexclude=a.lua;\\\nenabled=false\n";
    let edited = with_enabled(continued, false);
    assert!(edited.starts_with(continued));
    assert!(!Profile::parse("x", &edited).unwrap().enabled);
}

#[test]
fn one_unreadable_profile_does_not_hide_the_others() {
    let directory = std::env::temp_dir().join(format!("luabuddy-each-{}", std::process::id()));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("A.properties"), "id=A\n").unwrap();
    fs::write(directory.join("Bad.properties"), "id=Bad\nenabeld=true\n").unwrap();
    fs::write(directory.join("Copy.properties"), "id=A\n").unwrap();
    fs::write(directory.join("C.properties"), "id=C\n").unwrap();
    let (profiles, errors) = luabuddy_core::profile::read_each_profile(&directory).unwrap();
    assert_eq!(
        profiles
            .iter()
            .map(|profile| profile.id.as_str())
            .collect::<Vec<_>>(),
        ["A", "C"]
    );
    assert_eq!(errors.len(), 2);
    assert!(errors.iter().any(|error| error.message.contains("enabeld")));
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("Duplicate profile for A"))
    );
    assert!(read_profiles(&directory).is_err());
    fs::remove_dir_all(&directory).unwrap();
}

#[test]
fn source_folders_match_without_regard_to_case() {
    let root = std::env::temp_dir().join(format!("luabuddy-case-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("Media/LUA/Shared/Sub")).unwrap();
    fs::write(root.join("Media/LUA/Shared/Sub/Thing.lua"), "return 1").unwrap();
    let profile = Profile::parse(root.join("p.properties"), "id=Case\n").unwrap();
    let sources = scan_sources(&profile, std::slice::from_ref(&root), &BTreeSet::new()).unwrap();
    assert_eq!(sources[0].name, "shared/Sub/Thing.lua");
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_final_continuation_does_not_swallow_the_appended_enabled_line() {
    use luabuddy_core::profile::with_enabled;
    // By hand from the properties format: the backslash joins the next line to
    // `requires`, so `enabled=false` must be separated by a blank line.
    for text in ["id=foo\nrequires=a;\\\n", "id=foo\nrequires=a;\\"] {
        let edited = with_enabled(text, false);
        let profile = Profile::parse("x", &edited).unwrap();
        assert!(!profile.enabled, "{edited:?}");
        assert_eq!(profile.requires, [id("a")]);
        let restored = Profile::parse("x", &with_enabled(&edited, true)).unwrap();
        assert!(restored.enabled);
        assert_eq!(restored.requires, [id("a")]);
    }
}

#[test]
fn repeated_selection_toggles_leave_the_profile_the_same_size() {
    use luabuddy_core::selection::{self, Lifetime};
    let root = Directory::new();
    let file = root.0.join("Toggled.properties");
    fs::write(&file, "# mine\nid=Toggled\nexclude=a.lua;\\\n b.lua\\").unwrap();
    let toggle = |on: bool| {
        if on {
            selection::select(&root.0, &id("Toggled"), None, Lifetime::Persistent, None).unwrap();
        } else {
            assert!(selection::deselect(&root.0, &id("Toggled"), Lifetime::Persistent).unwrap());
        }
        fs::metadata(&file).unwrap().len()
    };
    let off = toggle(false);
    let on = toggle(true);
    for _ in 0..5 {
        assert_eq!(toggle(false), off);
        assert_eq!(toggle(true), on);
    }
    let profile = read_profiles(&root.0).unwrap().remove(0);
    assert!(profile.enabled);
    assert_eq!(profile.exclude, ["a.lua", "b.lua"]);
}

#[test]
fn queued_and_skipped_registrations_are_disjoint() {
    use luabuddy_core::selection::{self, Lifetime};
    let root = Directory::new();
    selection::select(&root.0, &id("Conflicted"), None, Lifetime::Once, None).unwrap();
    selection::select(&root.0, &id("Usable"), None, Lifetime::Once, None).unwrap();
    fs::write(
        root.0.join("user.properties"),
        "id=Conflicted\nenabled=false\n",
    )
    .unwrap();
    let (usable, skipped) = selection::queue(&root.0).unwrap();
    assert_eq!(
        usable.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
        ["Usable"]
    );
    assert_eq!(skipped.len(), 1);
    assert!(skipped[0].message.contains("Conflicted"));
    assert_eq!(selection::pending(&root.0).unwrap(), usable);
    assert_eq!(selection::skipped(&root.0).unwrap(), skipped);
}

#[test]
fn mod_ids_reject_control_characters_and_profiles_have_one_size_limit() {
    use luabuddy_core::profile::{PROFILE_LIMIT, read_text};
    for bad in ["a\u{7}b", "a\tb", "a\u{85}b", "a\0b", "\n", ""] {
        assert!(ModId::new(bad).is_err(), "{bad:?}");
    }
    assert_eq!(ModId::new("  Mod \n").unwrap().as_str(), "Mod");
    let root = Directory::new();
    let big = root.0.join("Big.properties");
    fs::write(
        &big,
        format!("id=Big\n#{}\n", "x".repeat(PROFILE_LIMIT as usize)),
    )
    .unwrap();
    assert!(read_text(&big).is_err());
    assert!(read_profiles(&root.0).is_err());
    fs::write(&big, format!("id=Big\n#{}\n", "x".repeat(1000))).unwrap();
    assert_eq!(read_profiles(&root.0).unwrap().len(), 1);
}
