use luabuddy_patcher::*;
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, Weak};

struct FixtureBackend {
    capabilities: Capabilities,
    installed: Vec<String>,
    removed: Vec<u64>,
    stale: bool,
    fail_install: Option<String>,
    fail_remove: bool,
}
impl Default for FixtureBackend {
    fn default() -> Self {
        Self {
            capabilities: Capabilities {
                hooks: BTreeSet::from([
                    HookKind::Before,
                    HookKind::Replace,
                    HookKind::ModifyArguments,
                    HookKind::ModifyResult,
                    HookKind::AfterNormalReturn,
                ]),
                reversible: true,
            },
            installed: vec![],
            removed: vec![],
            stale: false,
            fail_install: None,
            fail_remove: false,
        }
    }
}
impl PatchBackend for FixtureBackend {
    fn capabilities(&self) -> Capabilities {
        self.capabilities.clone()
    }
    fn inspect(&mut self, target: &Target) -> Result<ObservedTarget> {
        let mut target = target.clone();
        if self.stale {
            target.bytecode_sha256 = bytecode_sha256(b"different method")
        }
        Ok(ObservedTarget { target })
    }
    fn install(&mut self, spec: &PatchSpec) -> Result<InstallReceipt> {
        if self.fail_install.as_deref() == Some(&spec.id) {
            return Err(Error::Backend("injected install failure".into()));
        }
        self.installed.push(spec.id.clone());
        Ok(InstallReceipt {
            token: self.installed.len() as u64,
        })
    }
    fn uninstall(&mut self, receipt: &InstallReceipt) -> Result<()> {
        if self.fail_remove {
            return Err(Error::Backend("injected uninstall failure".into()));
        }
        self.removed.push(receipt.token);
        Ok(())
    }
}
fn spec(id: &str, kind: HookKind) -> PatchSpec {
    PatchSpec {
        id: id.into(),
        owner: "fixture".into(),
        target: Target {
            class: "fixture/Example".into(),
            method: "answer".into(),
            descriptor: "(I)I".into(),
            loader: "fixture-loader".into(),
            bytecode_sha256: bytecode_sha256(&[0x1a, 0xac]),
        },
        kind,
        requires: vec![],
        priority: 0,
        reentrancy: Reentrancy::SkipSelf,
    }
}
fn invocation(target: Target) -> Invocation {
    Invocation {
        target,
        stage: Stage::Entry,
        arguments: vec![Value::Int(7)],
        result: None,
        exception: None,
    }
}
fn observe(_: &mut Invocation) -> Result<HookControl> {
    Ok(HookControl::Continue)
}

#[test]
fn target_validation_rejects_wrong_descriptors_and_unbounded_arrays() {
    for descriptor in [
        "I",
        "(V)I",
        "()",
        "([V)V",
        "(Ljava.lang.String;)V",
        "(I)Vextra",
    ] {
        let mut target = spec("target", HookKind::Before).target;
        target.descriptor = descriptor.into();
        assert!(target.validate().is_err(), "{descriptor}");
    }
    let mut target = spec("target", HookKind::Before).target;
    target.descriptor = format!("({}I)V", "[".repeat(256));
    assert!(target.validate().is_err());
    target.descriptor = "(Ljava/lang/String;[[IJD)Ljava/lang/Object;".into();
    assert!(target.validate().is_ok());
}

#[test]
fn registration_is_not_installation_and_duplicate_ids_fail() {
    let mut registry = Registry::default();
    registry
        .register(spec("patch", HookKind::Before), Arc::new(observe))
        .unwrap();
    assert_eq!(registry.diagnostics()[0].state, PatchState::Registered);
    assert!(matches!(
        registry.register(spec("patch", HookKind::Before), Arc::new(observe)),
        Err(Error::Duplicate(_))
    ));
}

#[test]
fn dependency_order_beats_priority_and_ties_use_stable_ids() {
    let mut registry = Registry::default();
    let mut first = spec("a", HookKind::Before);
    first.priority = 100;
    let mut dependent = spec("b", HookKind::Before);
    dependent.priority = -100;
    dependent.requires = vec!["a".into()];
    registry.register(first, Arc::new(observe)).unwrap();
    registry.register(dependent, Arc::new(observe)).unwrap();
    registry
        .register(spec("c", HookKind::Before), Arc::new(observe))
        .unwrap();
    let mut backend = FixtureBackend::default();
    assert_eq!(
        registry
            .activate(&["b".into(), "c".into()], &mut backend)
            .unwrap(),
        ["c", "a", "b"]
    );
    assert_eq!(backend.installed, ["c", "a", "b"]);
    assert!(
        registry
            .activate(&["b".into()], &mut backend)
            .unwrap()
            .is_empty()
    );
    assert!(registry.disable("a", &mut backend).is_err());
    registry.disable("b", &mut backend).unwrap();
    registry.disable("a", &mut backend).unwrap();
    registry.disable("a", &mut backend).unwrap();
    assert_eq!(backend.removed, [3, 2]);
}

#[test]
fn cycles_missing_dependencies_conflicts_and_unsupported_hooks_fail_before_mutation() {
    let mut registry = Registry::default();
    let mut a = spec("a", HookKind::Before);
    a.requires = vec!["b".into()];
    let mut b = spec("b", HookKind::Before);
    b.requires = vec!["a".into()];
    registry.register(a, Arc::new(observe)).unwrap();
    let mut backend = FixtureBackend::default();
    assert!(matches!(
        registry.activate(&["a".into()], &mut backend),
        Err(Error::MissingDependency(_))
    ));
    registry.register(b, Arc::new(observe)).unwrap();
    assert!(matches!(
        registry.activate(&["a".into()], &mut backend),
        Err(Error::Cycle(_))
    ));
    assert!(backend.installed.is_empty());
    let mut registry = Registry::default();
    registry
        .register(spec("a", HookKind::Replace), Arc::new(observe))
        .unwrap();
    registry
        .register(spec("b", HookKind::Replace), Arc::new(observe))
        .unwrap();
    assert!(matches!(
        registry.activate(&["a".into(), "b".into()], &mut backend),
        Err(Error::Conflict(_))
    ));
    let mut registry = Registry::default();
    registry
        .register(spec("around", HookKind::Around), Arc::new(observe))
        .unwrap();
    backend.capabilities.hooks.insert(HookKind::Around);
    assert!(matches!(
        registry.activate(&["around".into()], &mut backend),
        Err(Error::Unsupported(_))
    ));
    assert!(backend.installed.is_empty());
}

#[test]
fn stale_bytecode_rejects_whole_batch_before_install() {
    let mut registry = Registry::default();
    registry
        .register(spec("a", HookKind::Before), Arc::new(observe))
        .unwrap();
    registry
        .register(spec("b", HookKind::Before), Arc::new(observe))
        .unwrap();
    let mut backend = FixtureBackend {
        stale: true,
        ..Default::default()
    };
    assert!(matches!(
        registry.activate(&["a".into(), "b".into()], &mut backend),
        Err(Error::StaleTarget(_))
    ));
    assert!(backend.installed.is_empty());
    assert!(
        registry
            .diagnostics()
            .iter()
            .all(|item| matches!(item.state, PatchState::Rejected(_)))
    );
}

#[test]
fn failed_install_rolls_back_previous_receipts_and_quarantines_uncertain_rollback() {
    for fail_remove in [false, true] {
        let mut registry = Registry::default();
        registry
            .register(spec("a", HookKind::Before), Arc::new(observe))
            .unwrap();
        registry
            .register(spec("b", HookKind::Before), Arc::new(observe))
            .unwrap();
        let mut backend = FixtureBackend {
            fail_install: Some("b".into()),
            fail_remove,
            ..Default::default()
        };
        assert!(
            registry
                .activate(&["a".into(), "b".into()], &mut backend)
                .is_err()
        );
        let states = registry.diagnostics();
        assert!(matches!(states[1].state, PatchState::Failed(_)));
        if fail_remove {
            assert!(matches!(states[0].state, PatchState::Quarantined(_)))
        } else {
            assert_eq!(states[0].state, PatchState::Registered);
            assert_eq!(backend.removed, [1])
        }
    }
}

#[test]
fn replacement_checks_type_and_undeclared_mutations_restore_frame() {
    let mut registry = Registry::default();
    let patch = spec("replace", HookKind::Replace);
    let target = patch.target.clone();
    registry
        .register(
            patch,
            Arc::new(|_: &mut Invocation| Ok(HookControl::Return(Value::Int(42)))),
        )
        .unwrap();
    registry
        .activate(&["replace".into()], &mut FixtureBackend::default())
        .unwrap();
    assert_eq!(
        registry.dispatch(&mut invocation(target.clone())).unwrap(),
        HookControl::Return(Value::Int(42))
    );
    let mut registry = Registry::default();
    registry
        .register(
            spec("bad", HookKind::Before),
            Arc::new(|call: &mut Invocation| {
                call.arguments[0] = Value::Int(99);
                Ok(HookControl::Continue)
            }),
        )
        .unwrap();
    registry
        .activate(&["bad".into()], &mut FixtureBackend::default())
        .unwrap();
    let mut call = invocation(target.clone());
    assert!(registry.dispatch(&mut call).is_err());
    assert_eq!(call.arguments, [Value::Int(7)]);
    let mut registry = Registry::default();
    registry
        .register(
            spec("bad-type", HookKind::Replace),
            Arc::new(|_: &mut Invocation| Ok(HookControl::Return(Value::Long(42)))),
        )
        .unwrap();
    registry
        .activate(&["bad-type".into()], &mut FixtureBackend::default())
        .unwrap();
    assert!(registry.dispatch(&mut invocation(target)).is_err());
}

#[test]
fn argument_and_result_edits_obey_declared_semantics() {
    let mut registry = Registry::default();
    let target = spec("arguments", HookKind::ModifyArguments).target;
    registry
        .register(
            spec("arguments", HookKind::ModifyArguments),
            Arc::new(|call: &mut Invocation| {
                call.arguments[0] = Value::Int(11);
                Ok(HookControl::Continue)
            }),
        )
        .unwrap();
    registry
        .register(
            spec("result", HookKind::ModifyResult),
            Arc::new(|call: &mut Invocation| {
                call.result = Some(Value::Int(99));
                Ok(HookControl::Continue)
            }),
        )
        .unwrap();
    registry
        .activate(
            &["arguments".into(), "result".into()],
            &mut FixtureBackend::default(),
        )
        .unwrap();
    let mut call = invocation(target);
    registry.dispatch(&mut call).unwrap();
    assert_eq!(call.arguments, [Value::Int(11)]);
    call.stage = Stage::NormalReturn;
    call.result = Some(Value::Int(11));
    registry.dispatch(&mut call).unwrap();
    assert_eq!(call.result, Some(Value::Int(99)));
}

#[test]
fn hook_panics_do_not_escape_and_cleanup_allows_next_call() {
    let mut registry = Registry::default();
    let target = spec("panic", HookKind::Before).target;
    registry
        .register(
            spec("panic", HookKind::Before),
            Arc::new(|call: &mut Invocation| -> Result<HookControl> {
                call.arguments[0] = Value::Int(100);
                panic!("fixture panic")
            }),
        )
        .unwrap();
    registry
        .activate(&["panic".into()], &mut FixtureBackend::default())
        .unwrap();
    for _ in 0..2 {
        let mut call = invocation(target.clone());
        assert!(matches!(registry.dispatch(&mut call), Err(Error::Panic(_))));
        assert_eq!(call.arguments, [Value::Int(7)]);
    }
}

#[test]
fn nested_calls_skip_self_or_reject_as_declared() {
    for policy in [Reentrancy::SkipSelf, Reentrancy::Reject] {
        let holder: Arc<OnceLock<Weak<Registry>>> = Arc::new(OnceLock::new());
        let callback_holder = holder.clone();
        let mut patch = spec("nested", HookKind::Before);
        patch.reentrancy = policy;
        let target = patch.target.clone();
        let mut registry = Registry::default();
        registry
            .register(
                patch,
                Arc::new(move |call: &mut Invocation| {
                    callback_holder
                        .get()
                        .unwrap()
                        .upgrade()
                        .unwrap()
                        .dispatch(call)
                }),
            )
            .unwrap();
        registry
            .activate(&["nested".into()], &mut FixtureBackend::default())
            .unwrap();
        let registry = Arc::new(registry);
        holder.set(Arc::downgrade(&registry)).unwrap();
        let result = registry.dispatch(&mut invocation(target));
        match policy {
            Reentrancy::SkipSelf => assert_eq!(result.unwrap(), HookControl::Continue),
            Reentrancy::Reject => assert!(matches!(result, Err(Error::Reentrant(_)))),
        }
    }
}

#[test]
fn concurrent_calls_keep_independent_reentrancy_state() {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let mut registry = Registry::default();
    let target = spec("concurrent", HookKind::Before).target;
    registry
        .register(
            spec("concurrent", HookKind::Before),
            Arc::new(move |_: &mut Invocation| {
                observed.fetch_add(1, Ordering::SeqCst);
                Ok(HookControl::Continue)
            }),
        )
        .unwrap();
    registry
        .activate(&["concurrent".into()], &mut FixtureBackend::default())
        .unwrap();
    std::thread::scope(|scope| {
        for _ in 0..8 {
            let registry = &registry;
            let target = &target;
            scope.spawn(move || {
                for _ in 0..100 {
                    registry.dispatch(&mut invocation(target.clone())).unwrap();
                }
            });
        }
    });
    assert_eq!(calls.load(Ordering::SeqCst), 800);
}

#[test]
fn independent_registries_do_not_share_local_patch_reentrancy_names() {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&calls);
    let mut inner = Registry::default();
    let mut inner_patch = spec("shared", HookKind::Before);
    inner_patch.owner = "inner-owner".into();
    inner_patch.target.class = "fixture/Inner".into();
    let inner_target = inner_patch.target.clone();
    inner
        .register(
            inner_patch,
            Arc::new(move |_: &mut Invocation| {
                observed.fetch_add(1, Ordering::SeqCst);
                Ok(HookControl::Continue)
            }),
        )
        .unwrap();
    inner
        .activate(&["shared".into()], &mut FixtureBackend::default())
        .unwrap();
    let inner = Arc::new(inner);
    let mut outer = Registry::default();
    let mut outer_patch = spec("shared", HookKind::Before);
    outer_patch.owner = "outer-owner".into();
    outer_patch.target.class = "fixture/Outer".into();
    let outer_target = outer_patch.target.clone();
    outer
        .register(
            outer_patch,
            Arc::new(move |_: &mut Invocation| {
                inner.dispatch(&mut invocation(inner_target.clone()))
            }),
        )
        .unwrap();
    outer
        .activate(&["shared".into()], &mut FixtureBackend::default())
        .unwrap();
    outer.dispatch(&mut invocation(outer_target)).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn registry_snapshots_keep_the_same_reentrancy_identity() {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&calls);
    let snapshot = Arc::new(OnceLock::<Weak<Registry>>::new());
    let nested_snapshot = Arc::clone(&snapshot);
    let patch = spec("snapshot", HookKind::Before);
    let target = patch.target.clone();
    let nested_target = target.clone();
    let mut registry = Registry::default();
    registry
        .register(
            patch,
            Arc::new(move |_: &mut Invocation| {
                if observed.fetch_add(1, Ordering::SeqCst) == 0 {
                    nested_snapshot
                        .get()
                        .unwrap()
                        .upgrade()
                        .unwrap()
                        .dispatch(&mut invocation(nested_target.clone()))?;
                }
                Ok(HookControl::Continue)
            }),
        )
        .unwrap();
    registry
        .activate(&["snapshot".into()], &mut FixtureBackend::default())
        .unwrap();
    let clone = Arc::new(registry.clone());
    snapshot.set(Arc::downgrade(&clone)).unwrap();
    registry.dispatch(&mut invocation(target)).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn nan_arguments_remain_bit_identical_for_observers() {
    let mut registry = Registry::default();
    let mut patch = spec("nan", HookKind::Before);
    patch.target.descriptor = "(D)V".into();
    let target = patch.target.clone();
    registry.register(patch, Arc::new(observe)).unwrap();
    registry
        .activate(&["nan".into()], &mut FixtureBackend::default())
        .unwrap();
    let mut call = Invocation {
        target,
        stage: Stage::Entry,
        arguments: vec![Value::Double(f64::from_bits(0x7ff8_0000_0000_0042))],
        result: None,
        exception: None,
    };
    assert_eq!(registry.dispatch(&mut call).unwrap(), HookControl::Continue);
}

#[test]
fn dispatch_follows_dependencies_before_priority_and_reverses_normal_returns() {
    let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut registry = Registry::default();
    let target = spec("entry-a", HookKind::Before).target;
    for (id, dependency, priority, kind) in [
        ("entry-a", None, 100, HookKind::Before),
        ("entry-b", Some("entry-a"), -100, HookKind::Before),
        ("exit-a", None, 100, HookKind::AfterNormalReturn),
        ("exit-b", Some("exit-a"), -100, HookKind::AfterNormalReturn),
    ] {
        let mut patch = spec(id, kind);
        patch.requires = dependency.into_iter().map(str::to_owned).collect();
        patch.priority = priority;
        let observed = observed.clone();
        registry
            .register(
                patch,
                Arc::new(move |_: &mut Invocation| {
                    observed.lock().unwrap().push(id);
                    Ok(HookControl::Continue)
                }),
            )
            .unwrap();
    }
    registry
        .activate(
            &["entry-b".into(), "exit-b".into()],
            &mut FixtureBackend::default(),
        )
        .unwrap();
    let mut call = invocation(target);
    registry.dispatch(&mut call).unwrap();
    call.stage = Stage::NormalReturn;
    call.result = Some(Value::Int(7));
    registry.dispatch(&mut call).unwrap();
    assert_eq!(
        *observed.lock().unwrap(),
        ["entry-a", "entry-b", "exit-b", "exit-a"]
    );
}

#[test]
fn uncertain_receipts_block_new_activation_until_removed() {
    let mut registry = Registry::default();
    registry
        .register(spec("a", HookKind::Before), Arc::new(observe))
        .unwrap();
    registry
        .register(spec("b", HookKind::Before), Arc::new(observe))
        .unwrap();
    registry
        .register(spec("c", HookKind::Before), Arc::new(observe))
        .unwrap();
    let mut backend = FixtureBackend {
        fail_install: Some("b".into()),
        fail_remove: true,
        ..Default::default()
    };
    assert!(
        registry
            .activate(&["a".into(), "b".into()], &mut backend)
            .is_err()
    );
    assert!(matches!(
        registry.activate(&["c".into()], &mut backend),
        Err(Error::Conflict(_))
    ));
    assert_eq!(backend.installed, ["a"]);
    backend.fail_remove = false;
    registry.disable("a", &mut backend).unwrap();
    assert_eq!(
        registry.activate(&["c".into()], &mut backend).unwrap(),
        ["c"]
    );
}
