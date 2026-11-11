use luabuddy_core::lifecycle::{
    Candidate, CleanupDecision, CleanupOutcome, GenerationState, Operation, RequestContext, Session,
};
use luabuddy_core::management::{
    ActivationState, DownloadState, ManagedMod, Observation, PendingAction,
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
        Self(path)
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
fn management_subscription_download_activation_and_disable_have_distinct_verified_states() {
    let mut state = ManagedMod {
        id: id("A"),
        workshop: Some(10),
        download: DownloadState::Unsubscribed,
        activation: ActivationState::Disabled,
        requires: BTreeSet::new(),
        pending: None,
    };
    let available = BTreeSet::new();
    state.request(PendingAction::Subscribe, &available).unwrap();
    state.request(PendingAction::Subscribe, &available).unwrap();
    assert_eq!(state.download, DownloadState::Unsubscribed);
    assert!(
        state
            .observe(Observation::LoadedVerified { generation: 1 })
            .is_err()
    );
    state.observe(Observation::SubscriptionVerified).unwrap();
    assert_eq!(state.download, DownloadState::Subscribed);
    state.request(PendingAction::Download, &available).unwrap();
    state
        .observe(Observation::DownloadProgress {
            downloaded: 2,
            total: Some(10),
        })
        .unwrap();
    assert!(matches!(
        state.download,
        DownloadState::Downloading { downloaded: 2, .. }
    ));
    assert!(
        state
            .observe(Observation::DownloadProgress {
                downloaded: 11,
                total: Some(10)
            })
            .is_err()
    );
    state.observe(Observation::InstallationVerified).unwrap();
    assert_eq!(state.download, DownloadState::Installed);
    state.request(PendingAction::Enable, &available).unwrap();
    state.observe(Observation::EnabledVerified).unwrap();
    state.request(PendingAction::Load, &available).unwrap();
    state
        .observe(Observation::LoadedVerified { generation: 3 })
        .unwrap();
    assert_eq!(state.activation, ActivationState::Loaded { generation: 3 });
    state.request(PendingAction::Disable, &available).unwrap();
    state.observe(Observation::DisabledVerified).unwrap();
    assert!(matches!(
        state.activation,
        ActivationState::ReconnectRequired { .. }
    ));
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
    let selected = selection::select(&root.0, &id("Kept"), None, Lifetime::Persistent).unwrap();
    let mut expected = before.clone();
    expected.enabled = true;
    assert_eq!(selected, expected);
    assert!(selection::deselect(&root.0, &id("Kept"), Lifetime::Persistent).unwrap());
    assert_eq!(read_profiles(&root.0).unwrap(), vec![before]);
    let queued =
        selection::select(&root.0, &id("Temporary ü"), Some(&source), Lifetime::Once).unwrap();
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
    assert!(selection::select(&root.0, &id("Kept"), None, Lifetime::Once).is_err());
    selection::select(&root.0, &id("Temporary"), None, Lifetime::Once).unwrap();
    assert!(selection::select(&root.0, &id("Temporary"), None, Lifetime::Persistent).is_err());
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
fn one_launch_refuses_a_persistent_selection_created_after_queuing() {
    use luabuddy_core::selection::{self, Lifetime};
    let root = Directory::new();
    let source = root.0.join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("original.txt"), "keep this source").unwrap();
    selection::select(&root.0, &id("Temporary"), Some(&source), Lifetime::Once).unwrap();
    let persistent = root.0.join("user.properties");
    let settings = b"id=Temporary\nenabled=false\nmount=true\n";
    fs::write(&persistent, settings).unwrap();
    let error = selection::claim(&root.0).unwrap_err();
    assert!(
        error
            .message
            .contains("conflicts with a persistent profile")
    );
    assert_eq!(fs::read(&persistent).unwrap(), settings);
    assert_eq!(
        fs::read_to_string(source.join("original.txt")).unwrap(),
        "keep this source"
    );
    assert!(selection::pending(&root.0).unwrap().is_empty());
    assert!(selection::claim(&root.0).unwrap().is_empty());
}

#[test]
fn malformed_one_launch_registration_reports_its_retained_location() {
    use luabuddy_core::selection;
    let root = Directory::new();
    let persistent = root.0.join("user.properties");
    let settings = b"id=Kept\nenabled=true\n";
    fs::write(&persistent, settings).unwrap();
    let pending = root.0.join(".launch-once/pending");
    fs::create_dir_all(&pending).unwrap();
    fs::write(
        pending.join("Broken.properties"),
        "id=Broken\nunknown=true\n",
    )
    .unwrap();
    let error = selection::claim(&root.0).unwrap_err();
    let retained = error.path.unwrap();
    assert_eq!(
        fs::read_to_string(&retained).unwrap(),
        "id=Broken\nunknown=true\n"
    );
    assert_eq!(fs::read(&persistent).unwrap(), settings);
    assert!(selection::pending(&root.0).unwrap().is_empty());
    assert!(selection::claim(&root.0).unwrap().is_empty());
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
    assert!(selection::claim(&root.0).is_err());
    assert_eq!(fs::read_to_string(&original).unwrap(), "id=Original\n");
    assert!(selection::claim(&root.0).unwrap().is_empty());
}
