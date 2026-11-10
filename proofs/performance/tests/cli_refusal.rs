use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    time::{SystemTime, UNIX_EPOCH},
};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "luabuddy-performance-cli-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        for path in [
            "game/projectzomboid.jar",
            "game/jre64/bin/java",
            "game/jre64/release",
            "runtime.so",
            "observer.so",
            "authority",
        ] {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"Unreachable launch fixture").unwrap();
        }
        Self(root)
    }

    fn run(&self, name: &str, extra: &[&str], bus: Option<&str>) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_luabuddy-performance"));
        command
            .arg("--game-dir")
            .arg(self.0.join("game"))
            .arg("--runtime")
            .arg(self.0.join("runtime.so"))
            .arg("--observer")
            .arg(self.0.join("observer.so"))
            .arg("--out")
            .arg(self.0.join(name))
            .args(["--single", "baseline", "--rounds", "1"])
            .args(extra)
            .env_remove("DBUS_SESSION_BUS_ADDRESS");
        if let Some(bus) = bus {
            command.env("DBUS_SESSION_BUS_ADDRESS", bus);
        }
        command.output().unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn missing_or_empty_bus_refuses_before_any_game_launch() {
    let fixture = Fixture::new();
    let authority = fixture.0.join("authority");
    for (name, bus) in [("missing-bus", None), ("empty-bus", Some(""))] {
        let result = fixture.run(
            name,
            &[
                "--frame-source",
                "mesa",
                "--display",
                ":99999",
                "--xauthority",
                authority.to_str().unwrap(),
            ],
            bus,
        );
        assert_eq!(result.status.code(), Some(1));
        assert!(
            String::from_utf8_lossy(&result.stderr).contains("DBUS_SESSION_BUS_ADDRESS"),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let out = fixture.0.join(name);
        let failure: Value =
            serde_json::from_slice(&fs::read(out.join("failure.json")).unwrap()).unwrap();
        assert_eq!(failure["completedRuns"], 0);
        let checks = fs::read_to_string(out.join("01-baseline/desktop-checks.jsonl")).unwrap();
        let record: Value = serde_json::from_str(checks.trim()).unwrap();
        assert_eq!(record["active"], Value::Null);
        assert_eq!(record["allowed"], false);
        assert_eq!(record["stage"], "workload setup");
        assert!(!out.join("01-baseline/owned-client.json").exists());
    }
}

#[test]
fn hardware_inputs_require_paired_display_and_favorites_context() {
    let fixture = Fixture::new();
    let authority = fixture.0.join("authority");
    for (name, extra, diagnostic) in [
        (
            "display-only",
            vec!["--display", ":99999"],
            "paired --display and --xauthority",
        ),
        (
            "authority-only",
            vec!["--xauthority", authority.to_str().unwrap()],
            "paired --display and --xauthority",
        ),
        (
            "mango-without-display",
            vec!["--frame-source", "mango_hud"],
            "explicit existing hardware display",
        ),
        (
            "favorite-without-context",
            vec!["--native-favorite"],
            "Native Favorites startup requires",
        ),
    ] {
        let result = fixture.run(name, &extra, None);
        assert_eq!(result.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&result.stderr).contains(diagnostic));
        assert!(
            !fixture
                .0
                .join(name)
                .join("01-baseline/owned-client.json")
                .exists()
        );
    }
}
