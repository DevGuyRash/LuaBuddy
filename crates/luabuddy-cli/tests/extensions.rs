#![cfg(target_os = "linux")]

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture {
    root: PathBuf,
    digest: String,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "luabuddy-extension-cli-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let artifact = b"metadata-only fixture; the installer must not load this as native code";
        let digest = format!("{:x}", Sha256::digest(artifact));
        fs::write(root.join("artifact.so"), artifact).unwrap();
        let manifest = json!({"schema_version":1,"abi_version":1,"id":"example","artifact_sha256":digest,"provenance":"independently reviewed fixture source","target_os":"linux","target_arch":std::env::consts::ARCH,"privileges":["observe_methods"],"patches":[{"id":"example/observe","owner":"example","kind":"before","target":{"class":"java/lang/Integer","method":"sum","descriptor":"(II)I","loader":"bootstrap","bytecode_sha256":"0".repeat(64)}}]});
        fs::write(
            root.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        Self { root, digest }
    }
    fn profiles(&self) -> PathBuf {
        self.root.join("config")
    }
    fn approval(&self) -> PathBuf {
        self.profiles().join("extensions/example/approval.json")
    }
    fn manifest_digest(&self) -> String {
        format!(
            "{:x}",
            Sha256::digest(fs::read(self.root.join("manifest.json")).unwrap())
        )
    }
    fn command(&self, name: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_luabuddy"));
        command.arg(name).arg("--profiles-dir").arg(self.profiles());
        command
    }
    fn approval_args(&self, command: &mut Command) {
        command.args([
            "--sha256",
            &self.digest,
            "--manifest-sha256",
            &self.manifest_digest(),
            "--provenance",
            "independently reviewed fixture source",
            "--allow",
            "observe_methods",
            "--trust-native-code",
        ]);
    }
    fn install(&self) -> Output {
        let mut command = self.command("extension-install");
        self.approval_args(&mut command);
        command
            .arg("--artifact")
            .arg(self.root.join("artifact.so"))
            .arg("--manifest")
            .arg(self.root.join("manifest.json"));
        command.output().unwrap()
    }
    fn enable(&self) -> Output {
        let mut command = self.command("extension-enable");
        self.approval_args(&mut command);
        command.args(["--extension-id", "example"]);
        command.output().unwrap()
    }
}
fn success(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn native_install_requires_separate_exact_enable_and_preserves_it_on_reinstall() {
    let fixture = Fixture::new();
    assert_eq!(success(fixture.install())["enabled"], false);
    let listed = success(fixture.command("extensions").output().unwrap());
    assert_eq!(listed["nativeCodeExecuted"], false);
    assert_eq!(listed["extensions"][0]["enabled"], false);
    assert_eq!(success(fixture.enable())["enabled"], true);
    let reinstalled = success(fixture.install());
    assert_eq!(reinstalled["enabled"], true);
    assert_eq!(reinstalled["changed"], false);
    let disabled = success(
        fixture
            .command("extension-disable")
            .args(["--extension-id", "example"])
            .output()
            .unwrap(),
    );
    assert_eq!(disabled["enabled"], false);
    assert_eq!(disabled["restartRequired"], true);
    assert_eq!(disabled["effective"], "next JVM process");
}

#[test]
fn missing_trust_and_changed_approval_cannot_activate_native_code() {
    let fixture = Fixture::new();
    let mut command = fixture.command("extension-install");
    command
        .arg("--artifact")
        .arg(fixture.root.join("artifact.so"))
        .arg("--manifest")
        .arg(fixture.root.join("manifest.json"))
        .arg("--manifest-sha256")
        .arg(fixture.manifest_digest())
        .args([
            "--sha256",
            &fixture.digest,
            "--provenance",
            "independently reviewed fixture source",
            "--allow",
            "observe_methods",
        ]);
    let output = command.output().unwrap();
    assert!(!output.status.success());
    assert!(!fixture.profiles().exists());
    success(fixture.install());
    let before = fs::read(fixture.approval()).unwrap();
    let output = fixture
        .command("extension-enable")
        .arg("--manifest-sha256")
        .arg(fixture.manifest_digest())
        .args([
            "--extension-id",
            "example",
            "--sha256",
            &fixture.digest,
            "--provenance",
            "different source",
            "--allow",
            "observe_methods",
            "--trust-native-code",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(before, fs::read(fixture.approval()).unwrap());
}

#[test]
fn modified_owned_artifact_is_preserved_and_refused() {
    let fixture = Fixture::new();
    success(fixture.install());
    let artifact = fixture
        .profiles()
        .join("extensions/example")
        .join(fixture.manifest_digest())
        .join("artifact.so");
    fs::write(&artifact, b"user edit").unwrap();
    assert!(!fixture.enable().status.success());
    assert!(!fixture.install().status.success());
    assert_eq!(fs::read(artifact).unwrap(), b"user edit");
}

#[test]
fn oversized_sources_fail_before_copying_or_recording_approval() {
    let fixture = Fixture::new();
    let file = fs::OpenOptions::new()
        .write(true)
        .open(fixture.root.join("artifact.so"))
        .unwrap();
    file.set_len(300 * 1024 * 1024).unwrap();
    assert!(!fixture.install().status.success());
    assert!(!fixture.profiles().exists());
}

#[test]
fn interrupted_approval_stage_is_preserved_before_retry() {
    let fixture = Fixture::new();
    success(fixture.install());
    let root = fixture.approval().parent().unwrap().to_owned();
    let stage = root.join(".approval.json.luabuddy-stage");
    let edited = b"interrupted or independently edited approval bytes";
    fs::write(&stage, edited).unwrap();
    assert_eq!(success(fixture.enable())["enabled"], true);
    assert!(!stage.exists());
    let backups = fs::read_dir(root.join("approval-history"))
        .unwrap()
        .map(|entry| fs::read(entry.unwrap().path()).unwrap())
        .collect::<Vec<_>>();
    assert!(backups.iter().any(|backup| backup == edited));
}

#[test]
fn one_invalid_approval_does_not_hide_healthy_native_entries() {
    let fixture = Fixture::new();
    success(fixture.install());
    let root = fixture.profiles().join("extensions/broken");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("approval.json"), b"invalid JSON").unwrap();
    let output = fixture.command("extensions").output().unwrap();
    assert!(!output.status.success());
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["extensions"].as_array().unwrap().len(), 2);
    assert!(
        result["extensions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["id"] == "example")
    );
    assert!(
        result["extensions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["state"] == "rejected")
    );
    assert_eq!(
        fs::read(root.join("approval.json")).unwrap(),
        b"invalid JSON"
    );
}

#[test]
fn metadata_update_with_unchanged_binary_requires_new_exact_approval() {
    let fixture = Fixture::new();
    success(fixture.install());
    success(fixture.enable());
    let old_manifest = fixture.manifest_digest();
    let old_root = fixture
        .profiles()
        .join("extensions/example")
        .join(&old_manifest);
    let before_manifest = fs::read(old_root.join("manifest.json")).unwrap();
    let before_artifact = fs::read(old_root.join("artifact.so")).unwrap();
    let mut replacement: Value = serde_json::from_slice(&before_manifest).unwrap();
    replacement["patches"][0]["priority"] = json!(17);
    fs::write(
        fixture.root.join("manifest.json"),
        serde_json::to_vec(&replacement).unwrap(),
    )
    .unwrap();
    let installed = success(fixture.install());
    assert_eq!(installed["enabled"], false);
    assert_eq!(installed["changed"], true);
    assert_ne!(old_manifest, fixture.manifest_digest());
    let approval_before = fs::read(fixture.approval()).unwrap();
    let rejected = fixture
        .command("extension-enable")
        .args([
            "--extension-id",
            "example",
            "--sha256",
            &fixture.digest,
            "--manifest-sha256",
            &old_manifest,
            "--provenance",
            "independently reviewed fixture source",
            "--allow",
            "observe_methods",
            "--trust-native-code",
        ])
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert_eq!(approval_before, fs::read(fixture.approval()).unwrap());
    assert_eq!(
        before_manifest,
        fs::read(old_root.join("manifest.json")).unwrap()
    );
    assert_eq!(
        before_artifact,
        fs::read(old_root.join("artifact.so")).unwrap()
    );
    let listed = success(fixture.command("extensions").output().unwrap());
    assert_eq!(
        listed["extensions"][0]["retainedGenerations"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(listed["extensions"][0]["enabled"], false);
    assert_eq!(success(fixture.enable())["enabled"], true);
}
