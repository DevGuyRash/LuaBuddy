use luabuddy_patcher::extension::*;
use luabuddy_patcher::*;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static FILE_ID: AtomicUsize = AtomicUsize::new(0);
struct Artifact(PathBuf);
impl Artifact {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "luabuddy-extension-approval-{}-{}",
            std::process::id(),
            FILE_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&path, b"fixture native artifact bytes; never executed").unwrap();
        Self(path)
    }
}
impl Drop for Artifact {
    fn drop(&mut self) {
        std::fs::remove_file(&self.0).unwrap();
    }
}
fn manifest() -> ExtensionManifest {
    ExtensionManifest {
        schema_version: 1,
        abi_version: ABI_VERSION,
        id: "fixture".into(),
        artifact_sha256: bytecode_sha256(b"fixture native artifact bytes; never executed"),
        provenance: "audited fixture source".into(),
        target_os: std::env::consts::OS.into(),
        target_arch: std::env::consts::ARCH.into(),
        privileges: BTreeSet::from([Privilege::ObserveMethods]),
        patches: vec![PatchSpec {
            id: "fixture/observe".into(),
            owner: "fixture".into(),
            target: Target {
                class: "fixture/Target".into(),
                method: "run".into(),
                descriptor: "()V".into(),
                loader: "fixture".into(),
                bytecode_sha256: bytecode_sha256(&[0xb1]),
            },
            kind: HookKind::Before,
            requires: vec![],
            priority: 0,
            reentrancy: Reentrancy::SkipSelf,
        }],
    }
}
fn consent(manifest: &ExtensionManifest) -> EnableConsent {
    EnableConsent {
        extension_id: manifest.id.clone(),
        artifact_sha256: manifest.artifact_sha256.clone(),
        provenance: manifest.provenance.clone(),
        allowed_privileges: manifest.privileges.clone(),
        install: true,
        enable: true,
    }
}

#[test]
fn exact_digest_provenance_and_two_intents_are_required_before_reading_artifact() {
    let manifest = manifest();
    for changed in [0, 1, 2, 3, 4] {
        let mut consent = consent(&manifest);
        match changed {
            0 => consent.install = false,
            1 => consent.enable = false,
            2 => consent.artifact_sha256 = "different".into(),
            3 => consent.provenance = "unapproved".into(),
            _ => consent.allowed_privileges.clear(),
        }
        assert!(matches!(
            verify(
                std::path::Path::new("file-that-does-not-exist"),
                manifest.clone(),
                &consent
            ),
            Err(Error::Permission(_))
        ));
    }
    let artifact = Artifact::new();
    assert!(verify(&artifact.0, manifest.clone(), &consent(&manifest)).is_ok());
    std::fs::write(&artifact.0, b"modified bytes").unwrap();
    assert!(matches!(
        verify(&artifact.0, manifest.clone(), &consent(&manifest)),
        Err(Error::Permission(_))
    ));
}

#[test]
fn wrong_platform_abi_owner_or_privilege_is_rejected() {
    let artifact = Artifact::new();
    for changed in 0..6 {
        let mut manifest = manifest();
        match changed {
            0 => manifest.abi_version = 900,
            1 => manifest.schema_version = 900,
            2 => manifest.target_arch = "some-other-process-architecture".into(),
            3 => manifest.patches[0].owner = "another-owner".into(),
            4 => manifest.patches[0].kind = HookKind::Replace,
            _ => manifest.patches.clear(),
        }
        assert!(verify(&artifact.0, manifest.clone(), &consent(&manifest)).is_err());
    }
}

#[test]
fn manifest_schema_rejects_unknown_fields() {
    let mut value = serde_json::to_value(manifest()).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .insert("execute_lua".into(), "arbitrary code".into());
    assert!(serde_json::from_value::<ExtensionManifest>(value).is_err());
}
