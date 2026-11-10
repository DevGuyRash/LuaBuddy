#![cfg(target_os = "linux")]

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    game: PathBuf,
    package: PathBuf,
    templates: PathBuf,
    runtime: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "luabuddy-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let game = root.join("Steam library 雪/projectzomboid");
        let templates = root.join("templates");
        fs::create_dir_all(game.join("jre64/bin")).unwrap();
        fs::create_dir(&templates).unwrap();
        fs::write(game.join("projectzomboid.jar"), []).unwrap();
        let java = std::env::split_paths(&std::env::var_os("PATH").unwrap())
            .map(|p| p.join("java"))
            .find(|p| p.is_file())
            .expect("Installer integration checks require an actual JVM on PATH");
        std::os::unix::fs::symlink(fs::canonicalize(java).unwrap(), game.join("jre64/bin/java"))
            .unwrap();
        fs::write(game.join("ProjectZomboid64.json"), serde_json::to_vec_pretty(&json!({"mainClass":"missing/InstallerFixture", "vmArgs":["-Xmx512m", "-Dunrelated=preserve me", "-javaagent:other.jar"], "custom":{"value":7}})).unwrap()).unwrap();
        fs::write(templates.join("Example.properties"), "id=Example\n").unwrap();
        let runtime = root.join("runtime.so");
        let mut elf = vec![0; 64];
        elf[..6].copy_from_slice(b"\x7fELF\x02\x01");
        elf[16..18].copy_from_slice(&3u16.to_le_bytes());
        elf[18..20].copy_from_slice(
            &(if cfg!(target_arch = "aarch64") {
                183u16
            } else {
                62u16
            })
            .to_le_bytes(),
        );
        fs::write(&runtime, elf).unwrap();
        let package = root.join("package-one");
        let fixture = Self {
            root,
            game,
            package,
            templates,
            runtime,
        };
        fixture.package(&fixture.package);
        fixture
    }
    fn command(&self, command: &str, args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_luabuddy"));
        cmd.arg(command);
        if matches!(
            command,
            "install"
                | "update"
                | "repair"
                | "verify"
                | "doctor"
                | "rollback"
                | "recover"
                | "uninstall"
        ) {
            cmd.arg("--game-dir").arg(self.game.parent().unwrap());
        }
        cmd.args(args).env_remove("PZ_GAME_DIR").output().unwrap()
    }
    fn success(&self, command: &str, args: &[&str]) -> Value {
        let result = self.command(command, args);
        assert!(
            result.status.success(),
            "{command}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        serde_json::from_slice(&result.stdout).unwrap()
    }
    fn package(&self, output: &Path) {
        self.success(
            "package",
            &[
                "--runtime",
                self.runtime.to_str().unwrap(),
                "--profiles-dir",
                self.templates.to_str().unwrap(),
                "--out",
                output.to_str().unwrap(),
            ],
        );
    }
    fn install(&self) -> Value {
        self.success("install", &["--package", self.package.to_str().unwrap()])
    }
    fn record(&self) -> Value {
        serde_json::from_slice(&fs::read(self.game.join(".luabuddy/native-install.json")).unwrap())
            .unwrap()
    }
    fn site(&self) -> Value {
        serde_json::from_slice(&fs::read(self.game.join("ProjectZomboid64.site.json")).unwrap())
            .unwrap()
    }
    fn profile_dir(&self) -> PathBuf {
        self.game.join(".luabuddy/profiles")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn empty_default_additive_selection_and_idempotence() {
    let f = Fixture::new();
    let base = fs::read(f.game.join("ProjectZomboid64.json")).unwrap();
    assert_eq!(f.install()["changed"], true);
    assert_eq!(fs::read_dir(f.profile_dir()).unwrap().count(), 0);
    let record = fs::read(f.game.join(".luabuddy/native-install.json")).unwrap();
    let site = fs::read(f.game.join("ProjectZomboid64.site.json")).unwrap();
    assert_eq!(f.install()["changed"], false);
    assert_eq!(
        record,
        fs::read(f.game.join(".luabuddy/native-install.json")).unwrap()
    );
    assert_eq!(
        site,
        fs::read(f.game.join("ProjectZomboid64.site.json")).unwrap()
    );
    assert_eq!(f.site()["custom"]["value"], 7);
    f.success(
        "install",
        &[
            "--package",
            f.package.to_str().unwrap(),
            "--mod",
            "Example",
            "--mod",
            "日本語 Workshop Mod",
        ],
    );
    let profiles = luabuddy_core::read_profiles(&f.profile_dir()).unwrap();
    assert_eq!(profiles.len(), 2);
    assert!(
        profiles
            .iter()
            .any(|p| p.id.as_str() == "日本語 Workshop Mod")
    );
    fs::write(
        f.profile_dir().join("Example.properties"),
        "id=Example\nenabled=false\n",
    )
    .unwrap();
    f.success(
        "install",
        &["--package", f.package.to_str().unwrap(), "--mod", "Example"],
    );
    assert_eq!(
        fs::read_to_string(f.profile_dir().join("Example.properties")).unwrap(),
        "id=Example\nenabled=false\n"
    );
    assert_eq!(
        base,
        fs::read(f.game.join("ProjectZomboid64.json")).unwrap()
    );
}

#[test]
fn package_rejects_corruption_and_unlisted_files() {
    let f = Fixture::new();
    f.success(
        "verify-package",
        &["--package", f.package.to_str().unwrap()],
    );
    fs::write(f.package.join("extra.so"), "unlisted").unwrap();
    assert_eq!(
        f.command(
            "verify-package",
            &["--package", f.package.to_str().unwrap()]
        )
        .status
        .code(),
        Some(2)
    );
    fs::remove_file(f.package.join("extra.so")).unwrap();
    fs::write(f.package.join("libluabuddy_host.so"), "changed").unwrap();
    assert_eq!(
        f.command(
            "verify-package",
            &["--package", f.package.to_str().unwrap()]
        )
        .status
        .code(),
        Some(2)
    );
    assert!(!f.game.join("ProjectZomboid64.site.json").exists());
}

#[test]
fn package_template_format_is_checked_before_activation_with_valid_manifest_hashes() {
    for (name, content) in [
        ("profiles/metadata.json", "{\"version\":1}\n"),
        ("profiles/nested/Extra.properties", "id=Extra\n"),
    ] {
        let f = Fixture::new();
        f.success(
            "verify-package",
            &["--package", f.package.to_str().unwrap()],
        );
        let path = f.package.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        let manifest_path = f.package.join("manifest.json");
        let mut manifest: Value =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        manifest["files"][name] = json!({
            "sha256":format!("{:x}", Sha256::digest(content.as_bytes())),
            "size":content.len(),
            "executable":false,
        });
        let mut manifest_bytes = serde_json::to_vec_pretty(&manifest).unwrap();
        manifest_bytes.push(b'\n');
        fs::write(&manifest_path, &manifest_bytes).unwrap();
        fs::write(
            f.package.join("manifest.sha256"),
            format!("{:x}  manifest.json\n", Sha256::digest(&manifest_bytes)),
        )
        .unwrap();
        let base = fs::read(f.game.join("ProjectZomboid64.json")).unwrap();
        for command in ["verify-package", "install"] {
            let result = f.command(command, &["--package", f.package.to_str().unwrap()]);
            assert_eq!(result.status.code(), Some(2));
            assert!(
                String::from_utf8_lossy(&result.stderr)
                    .contains("Unsupported profile template entry")
            );
        }
        assert_eq!(
            fs::read(f.game.join("ProjectZomboid64.json")).unwrap(),
            base
        );
        for path in [
            "ProjectZomboid64.site.json",
            ".luabuddy/native-install.json",
            ".luabuddy/native-transaction.json",
        ] {
            assert!(!f.game.join(path).exists());
        }
    }
}

#[test]
fn update_rollback_and_uninstall_preserve_user_edits() {
    let f = Fixture::new();
    f.install();
    let first = f.record()["current"]["package_digest"].clone();
    let mut before_update = f.site();
    before_update["custom"]["editedBeforeUpdate"] = json!(true);
    fs::write(
        f.game.join("ProjectZomboid64.site.json"),
        serde_json::to_vec(&before_update).unwrap(),
    )
    .unwrap();
    fs::write(f.templates.join("Second.properties"), "id=Second\n").unwrap();
    let second = f.root.join("package-two");
    f.package(&second);
    f.success(
        "update",
        &["--package", second.to_str().unwrap(), "--debug"],
    );
    assert_ne!(f.record()["current"]["package_digest"], first);
    f.success("rollback", &[]);
    assert_eq!(f.record()["current"]["package_digest"], first);
    assert_eq!(f.site()["custom"]["editedBeforeUpdate"], true);
    let mut site = f.site();
    site["custom"]["userEdited"] = json!(true);
    site["vmArgs"]
        .as_array_mut()
        .unwrap()
        .push(json!("-DnewUnrelated=keep"));
    fs::write(
        f.game.join("ProjectZomboid64.site.json"),
        serde_json::to_vec(&site).unwrap(),
    )
    .unwrap();
    assert_eq!(f.command("rollback", &[]).status.code(), Some(2));
    f.success("uninstall", &[]);
    let site = f.site();
    assert_eq!(site["custom"]["userEdited"], true);
    assert_eq!(site["custom"]["editedBeforeUpdate"], true);
    assert!(
        site["vmArgs"]
            .as_array()
            .unwrap()
            .contains(&json!("-DnewUnrelated=keep"))
    );
    assert!(
        !site["vmArgs"].as_array().unwrap().iter().any(|v| v
            .as_str()
            .unwrap()
            .starts_with("-agentpath:")
            || v.as_str().unwrap().starts_with("-Dluabuddy.config="))
    );
    assert_eq!(f.success("uninstall", &[])["changed"], false);
}

#[test]
fn rollback_restores_the_previous_profile_directory_for_reports_and_future_updates() {
    let f = Fixture::new();
    let a = f.root.join("profiles A 雪");
    let b = f.root.join("profiles B");
    fs::create_dir(&a).unwrap();
    fs::create_dir(&b).unwrap();
    let a_file = a.join("ChoiceA.properties");
    let b_file = b.join("ChoiceB.properties");
    let a_bytes = b"id=ChoiceA\nenabled=false\n";
    let b_bytes = b"id=ChoiceB\nenabled=true\n";
    fs::write(&a_file, a_bytes).unwrap();
    fs::write(&b_file, b_bytes).unwrap();
    let base = fs::read(f.game.join("ProjectZomboid64.json")).unwrap();
    for (command, directory) in [("install", &a), ("update", &b)] {
        f.success(
            command,
            &[
                "--package",
                f.package.to_str().unwrap(),
                "--profiles-dir",
                directory.to_str().unwrap(),
            ],
        );
    }
    let selected = |directory: &Path| {
        let argument = format!("-Dluabuddy.config={}", directory.display());
        let site = f.site();
        let args = site["vmArgs"].as_array().unwrap();
        assert_eq!(
            args.iter()
                .filter(|value| value.as_str().unwrap().starts_with("-Dluabuddy.config="))
                .collect::<Vec<_>>(),
            [&json!(argument)]
        );
        assert_eq!(f.record()["profiles"], json!(directory));
        assert_eq!(f.record()["current"]["config_argument"], json!(argument));
        assert!(args.contains(&json!("-Dunrelated=preserve me")));
        assert!(args.contains(&json!("-javaagent:other.jar")));
        assert_eq!(site["custom"]["value"], 7);
        for result in [
            f.success("verify", &[]),
            f.success(
                "doctor",
                &["--cache-dir", f.root.join("empty-cache").to_str().unwrap()],
            ),
        ] {
            assert_eq!(result["ok"], true);
            assert_eq!(result["profiles"], json!(directory));
        }
    };
    assert_eq!(f.success("rollback", &[])["profilesPreserved"], json!(a));
    selected(&a);
    f.success("update", &["--package", f.package.to_str().unwrap()]);
    selected(&a);
    for directory in [&b, &a] {
        assert_eq!(
            f.success("rollback", &[])["profilesPreserved"],
            json!(directory)
        );
        selected(directory);
    }
    f.success("uninstall", &[]);
    assert_eq!(fs::read(a_file).unwrap(), a_bytes);
    assert_eq!(fs::read(b_file).unwrap(), b_bytes);
    assert_eq!(
        fs::read(f.game.join("ProjectZomboid64.json")).unwrap(),
        base
    );
}

#[test]
fn rollback_refuses_invalid_effective_profile_arguments_without_changing_the_installation() {
    let f = Fixture::new();
    f.install();
    f.success(
        "update",
        &[
            "--package",
            f.package.to_str().unwrap(),
            "--profiles-dir",
            f.root.join("profiles-B").to_str().unwrap(),
        ],
    );
    let record = f.record();
    let record_path = f.game.join(".luabuddy/native-install.json");
    let site_path = f.game.join("ProjectZomboid64.site.json");
    let site = fs::read(&site_path).unwrap();
    for argument in [
        None,
        Some("-Dluabuddy.config".to_owned()),
        Some("-Dluabuddy.config=".to_owned()),
        Some("-Dluabuddy.config=relative".to_owned()),
        Some(format!("-Dluabuddy.config={}\n", f.root.display())),
        Some(format!("-Dluabuddy.config={}\r", f.root.display())),
        Some(format!("-Dluabuddy.config={}\0", f.root.display())),
    ] {
        let mut altered = record.clone();
        let prior_bytes: Vec<u8> =
            serde_json::from_value(altered["previous"]["site_bytes"].clone()).unwrap();
        let mut prior: Value = serde_json::from_slice(&prior_bytes).unwrap();
        let args = prior["vmArgs"].as_array_mut().unwrap();
        if let Some(argument) = argument {
            args.push(json!(argument));
        } else {
            args.retain(|value| !value.as_str().unwrap().starts_with("-Dluabuddy.config="));
        }
        altered["previous"]["site_bytes"] = json!(serde_json::to_vec(&prior).unwrap());
        let before = serde_json::to_vec(&altered).unwrap();
        fs::write(&record_path, &before).unwrap();
        let result = f.command("rollback", &[]);
        assert_eq!(result.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&result.stderr).contains("profile directory"));
        assert_eq!(fs::read(&site_path).unwrap(), site);
        assert_eq!(fs::read(&record_path).unwrap(), before);
        assert!(!f.game.join(".luabuddy/native-transaction.json").exists());
    }
}

#[test]
fn repair_retains_modified_runtime_and_preserves_user_profiles() {
    let f = Fixture::new();
    f.install();
    let runtime = PathBuf::from(f.record()["current"]["runtime"].as_str().unwrap());
    fs::write(&runtime, "user modified runtime").unwrap();
    assert_eq!(f.command("verify", &[]).status.code(), Some(1));
    assert_eq!(
        f.command("install", &["--package", f.package.to_str().unwrap()])
            .status
            .code(),
        Some(2)
    );
    f.success("repair", &["--package", f.package.to_str().unwrap()]);
    f.success("verify", &[]);
    let backups: Vec<_> = fs::read_dir(f.game.join(".luabuddy/recovery"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(backups.len(), 1);
    assert_eq!(fs::read(&backups[0]).unwrap(), b"user modified runtime");
    let profile = f.profile_dir().join("UserSettings.properties");
    let original = "id=UserSettings\nenabled=false\nmount=true\n# preserve this comment\n";
    fs::write(&profile, original).unwrap();
    f.install();
    f.success("verify", &[]);
    f.success("uninstall", &[]);
    assert_eq!(fs::read_to_string(profile).unwrap(), original);
    assert_eq!(fs::read(&backups[0]).unwrap(), b"user modified runtime");
}

#[test]
fn case_collisions_fail_before_activation() {
    let f = Fixture::new();
    let result = f.command(
        "install",
        &[
            "--package",
            f.package.to_str().unwrap(),
            "--mod",
            "Example",
            "--mod",
            "example",
        ],
    );
    assert_eq!(result.status.code(), Some(2));
    assert!(!f.game.join("ProjectZomboid64.site.json").exists());
    assert!(!f.game.join(".luabuddy/native-install.json").exists());
}

#[test]
fn immutable_template_repair_and_uninstall_preserve_user_files() {
    let f = Fixture::new();
    fs::create_dir_all(f.profile_dir()).unwrap();
    let original = "id=AutoAll\nenabled=false\nhook=AutoAll\n# preserve this comment\n";
    let profile_file = f.profile_dir().join("AutoAll.properties");
    fs::write(&profile_file, original).unwrap();
    f.install();
    let profile = luabuddy_core::read_profiles(&f.profile_dir())
        .unwrap()
        .remove(0);
    assert!(!profile.enabled);
    assert_eq!(profile.hook.as_deref(), Some("AutoAll"));
    assert_eq!(fs::read_to_string(&profile_file).unwrap(), original);
    let runtime = PathBuf::from(f.record()["current"]["runtime"].as_str().unwrap());
    let templates = runtime.parent().unwrap().join("profiles");
    assert_eq!(
        fs::read_to_string(templates.join("Example.properties")).unwrap(),
        "id=Example\n"
    );
    assert!(
        f.site()["vmArgs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str().unwrap().starts_with("-Dluabuddy.templates="))
    );
    fs::write(
        templates.join("Example.properties"),
        "user changed immutable template",
    )
    .unwrap();
    assert_eq!(f.command("verify", &[]).status.code(), Some(1));
    f.success("repair", &["--package", f.package.to_str().unwrap()]);
    assert_eq!(
        fs::read_to_string(templates.join("Example.properties")).unwrap(),
        "id=Example\n"
    );
    let repaired: Vec<_> = fs::read_dir(f.game.join(".luabuddy/recovery/templates"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(repaired.len(), 1);
    assert_eq!(
        fs::read_to_string(&repaired[0]).unwrap(),
        "user changed immutable template"
    );
    assert_eq!(fs::read_to_string(&profile_file).unwrap(), original);
    fs::write(
        templates.join("Example.properties"),
        "user changed immutable template",
    )
    .unwrap();
    f.success("uninstall", &[]);
    assert_eq!(
        fs::read_to_string(templates.join("Example.properties")).unwrap(),
        "user changed immutable template"
    );
    assert_eq!(fs::read_to_string(profile_file).unwrap(), original);
}

#[test]
fn interrupted_journal_recovers_every_applied_prefix_and_partial_stage() {
    let f = Fixture::new();
    fs::create_dir_all(f.profile_dir()).unwrap();
    let owned = f.game.join(".luabuddy/native/fault-owned");
    fs::create_dir_all(owned.parent().unwrap()).unwrap();
    let site = f.game.join("ProjectZomboid64.site.json");
    let before = b"before".to_vec();
    let after = b"after".to_vec();
    let old_site = serde_json::to_vec(&json!({"vmArgs":["-Dprevious=keep"]})).unwrap();
    let new_site = serde_json::to_vec(&json!({"vmArgs":["-Dnext=staged"]})).unwrap();
    for prefix in 0..=2 {
        fs::write(&owned, if prefix > 0 { &after } else { &before }).unwrap();
        fs::write(&site, if prefix > 1 { &new_site } else { &old_site }).unwrap();
        let stage = owned.with_file_name(".fault-owned.luabuddy-stage");
        fs::write(&stage, b"partial").unwrap();
        let journal = json!({"schema":1,"game":f.game,"profiles":f.profile_dir(),"changes":[{"path":owned,"before":before,"after":after,"executable":false},{"path":site,"before":old_site,"after":new_site,"executable":false}]});
        fs::write(
            f.game.join(".luabuddy/native-transaction.json"),
            serde_json::to_vec(&journal).unwrap(),
        )
        .unwrap();
        assert_eq!(f.success("recover", &[])["recovered"], true);
        assert_eq!(fs::read(&owned).unwrap(), before);
        assert_eq!(fs::read(&site).unwrap(), old_site);
        assert!(!stage.exists());
        assert!(!f.game.join(".luabuddy/native-transaction.json").exists());
    }
    let outside = f.root.join("foreign-file");
    fs::write(&outside, "foreign").unwrap();
    let malicious = json!({"schema":1,"game":f.game,"profiles":f.profile_dir(),"changes":[{"path":outside,"before":null,"after":b"foreign".to_vec(),"executable":false}]});
    fs::write(
        f.game.join(".luabuddy/native-transaction.json"),
        serde_json::to_vec(&malicious).unwrap(),
    )
    .unwrap();
    assert_eq!(f.command("recover", &[]).status.code(), Some(2));
    assert_eq!(fs::read_to_string(&outside).unwrap(), "foreign");
}

#[test]
fn diagnostics_reflect_failed_incomplete_invalid_and_stale_reports_without_writes() {
    let f = Fixture::new();
    f.install();
    let cache = f.root.join("private-cache");
    fs::create_dir_all(cache.join("LuaBuddy")).unwrap();
    let path = cache.join("LuaBuddy/diagnostics.json");
    let mut report = json!({"schemaVersion":1,"kind":"loader-initialization","version":"0.1.0","session":"00000000-0000-4000-8000-000000000001-1","operation":"startup","target":null,"gameVersion":"fixture","javaVersion":"25","configDirectory":f.profile_dir(),"startedAt":"2026-11-09T00:00:00Z","completedAt":"2026-11-09T00:00:01Z","durationMs":1000,"mods":{},"summary":{"loaded":0,"serverLoaded":0,"failed":0,"disabled":0,"pending":0},"error":null});
    fs::write(&path, serde_json::to_vec(&report).unwrap()).unwrap();
    assert_eq!(
        f.success("diagnostics", &["--cache-dir", cache.to_str().unwrap()])["reportStatus"],
        "completed"
    );
    report["configDirectory"] = json!(f.root.join("different-profile-selection"));
    fs::write(&path, serde_json::to_vec(&report).unwrap()).unwrap();
    let stale = f.success("doctor", &["--cache-dir", cache.to_str().unwrap()]);
    assert_eq!(stale["warnings"].as_array().unwrap().len(), 1);
    report["completedAt"] = Value::Null;
    report["durationMs"] = Value::Null;
    fs::write(&path, serde_json::to_vec(&report).unwrap()).unwrap();
    let incomplete = f.command("diagnostics", &["--cache-dir", cache.to_str().unwrap()]);
    assert_eq!(incomplete.status.code(), Some(1));
    assert_eq!(
        serde_json::from_slice::<Value>(&incomplete.stdout).unwrap()["reportStatus"],
        "incomplete"
    );
    report["error"] = json!("The requested generation was refused");
    fs::write(&path, serde_json::to_vec(&report).unwrap()).unwrap();
    let failed = f.command("diagnostics", &["--cache-dir", cache.to_str().unwrap()]);
    assert_eq!(failed.status.code(), Some(1));
    assert_eq!(
        serde_json::from_slice::<Value>(&failed.stdout).unwrap()["reportStatus"],
        "failed"
    );
    report["schemaVersion"] = json!(999);
    let invalid = serde_json::to_vec(&report).unwrap();
    fs::write(&path, &invalid).unwrap();
    assert_eq!(
        f.command("diagnostics", &["--cache-dir", cache.to_str().unwrap()])
            .status
            .code(),
        Some(2)
    );
    assert_eq!(fs::read(&path).unwrap(), invalid);
}

#[test]
fn a_package_without_bundled_profiles_keeps_a_usable_empty_template_directory() {
    let f = Fixture::new();
    fs::remove_file(f.templates.join("Example.properties")).unwrap();
    let package = f.root.join("empty-catalog-package");
    f.package(&package);
    f.success("install", &["--package", package.to_str().unwrap()]);
    let runtime = PathBuf::from(f.record()["current"]["runtime"].as_str().unwrap());
    let templates = runtime.parent().unwrap().join("profiles");
    assert!(templates.is_dir());
    assert!(luabuddy_core::read_profiles(&templates).unwrap().is_empty());
    assert!(
        luabuddy_core::read_profiles(&f.profile_dir())
            .unwrap()
            .is_empty()
    );
    fs::remove_dir(f.profile_dir()).unwrap();
    assert_eq!(f.command("verify", &[]).status.code(), Some(1));
}
