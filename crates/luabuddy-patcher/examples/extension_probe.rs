use luabuddy_patcher::extension::{
    EnableConsent, ExtensionManifest, LoadedExtension, Privilege, verify,
};
use luabuddy_patcher::*;
use std::collections::BTreeSet;
use std::path::Path;

#[derive(Default)]
struct ContractBackend(u64);
impl PatchBackend for ContractBackend {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            hooks: BTreeSet::from([HookKind::Before, HookKind::Replace]),
            reversible: true,
        }
    }
    fn inspect(&mut self, target: &Target) -> Result<ObservedTarget> {
        Ok(ObservedTarget {
            target: target.clone(),
        })
    }
    fn install(&mut self, _: &PatchSpec) -> Result<InstallReceipt> {
        self.0 += 1;
        Ok(InstallReceipt { token: self.0 })
    }
    fn uninstall(&mut self, _: &InstallReceipt) -> Result<()> {
        Ok(())
    }
}

fn load(
    path: &Path,
    id: &str,
    target: Target,
    kind: HookKind,
    privilege: Privilege,
    registry: &mut Registry,
) -> std::result::Result<String, Box<dyn std::error::Error>> {
    let hash = bytecode_sha256(&std::fs::read(path)?);
    let patch_id = format!("{id}/probe");
    let privileges = BTreeSet::from([privilege]);
    let manifest = ExtensionManifest {
        schema_version: 1,
        abi_version: 1,
        id: id.into(),
        artifact_sha256: hash.clone(),
        provenance: "repository-built Rust extension example".into(),
        target_os: std::env::consts::OS.into(),
        target_arch: std::env::consts::ARCH.into(),
        privileges: privileges.clone(),
        patches: vec![PatchSpec {
            id: patch_id.clone(),
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
        artifact_sha256: hash,
        provenance: manifest.provenance.clone(),
        allowed_privileges: privileges,
        install: true,
        enable: true,
    };
    let verified = verify(path, manifest, &consent)?;
    // SAFETY: These two libraries are built from the reviewed Rust examples in this
    // repository and remain unmodified for this single-process contract probe.
    unsafe { LoadedExtension::load(verified)? }.register(registry)?;
    Ok(patch_id)
}

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let paths: Vec<_> = std::env::args_os().skip(1).collect();
    if paths.len() != 2 {
        return Err("Supply the math-guard and string-observer library paths".into());
    }
    let math = Target {
        class: "java/lang/Math".into(),
        method: "abs".into(),
        descriptor: "(I)I".into(),
        loader: "contract-fixture".into(),
        bytecode_sha256: bytecode_sha256(b"math contract fixture"),
    };
    let string = Target {
        class: "java/lang/String".into(),
        method: "isEmpty".into(),
        descriptor: "()Z".into(),
        loader: "contract-fixture".into(),
        bytecode_sha256: bytecode_sha256(b"string contract fixture"),
    };
    let mut registry = Registry::default();
    let math_id = load(
        Path::new(&paths[0]),
        "math-guard",
        math.clone(),
        HookKind::Replace,
        Privilege::ReplaceMethods,
        &mut registry,
    )?;
    let string_id = load(
        Path::new(&paths[1]),
        "string-observer",
        string.clone(),
        HookKind::Before,
        Privilege::ObserveMethods,
        &mut registry,
    )?;
    let mut backend = ContractBackend::default();
    registry.activate(&[math_id.clone(), string_id.clone()], &mut backend)?;
    let mut invocation = Invocation {
        target: math,
        stage: Stage::Entry,
        arguments: vec![Value::Int(i32::MIN)],
        result: None,
        exception: None,
    };
    assert_eq!(
        registry.dispatch(&mut invocation)?,
        HookControl::Return(Value::Int(i32::MAX))
    );
    let mut observed = Invocation {
        target: string,
        stage: Stage::Entry,
        arguments: vec![],
        result: None,
        exception: None,
    };
    assert_eq!(registry.dispatch(&mut observed)?, HookControl::Continue);
    // SAFETY: This is the reviewed example library and scalar diagnostic ABI.
    let observer = unsafe { libloading::Library::new(&paths[1])? };
    // SAFETY: The reviewed example exports exactly this function signature.
    let calls =
        unsafe { observer.get::<unsafe extern "C" fn() -> u64>(b"luabuddy_observed_calls_v1\0")? };
    // SAFETY: The diagnostic takes no pointers and returns its atomic counter.
    assert_eq!(unsafe { calls() }, 1);
    registry.disable(&math_id, &mut backend)?;
    assert_eq!(registry.dispatch(&mut invocation)?, HookControl::Continue);
    registry.disable(&string_id, &mut backend)?;
    println!(
        "PASS two independent Rust extension ABI callbacks, digest approval, replacement, observation, and disable through a contract backend. JVM activation was not exercised."
    );
    Ok(())
}
