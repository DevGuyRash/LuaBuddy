use luabuddy::{
    InstallOptions, diagnostics, doctor, find_game, install, make_package, recover_install,
    rollback, steam_libraries, uninstall, verify_install, verify_package,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::path::PathBuf;

mod setup;

const HELP: &str = "LuaBuddy native installer\n\nCommands:\n  setup [--package DIR] [--game-dir DIR]         Open graphical native setup on Linux.\n  discover                                      Find the installed game and Steam libraries.\n  package --runtime FILE --profiles-dir DIR --out DIR\n                                                Create a native package and integrity manifest.\n  verify-package --package DIR                  Check all package files and native architecture.\n  install --package DIR [--mod ID]               Install without selecting any default mods.\n  update --package DIR                          Activate a verified package and retain rollback.\n  repair --package DIR                          Repair owned runtime files, preserving edits.\n  verify | doctor | status                      Inspect the installed files without changing them.\n  rollback                                      Restore the previous owned native generation.\n  recover                                       Undo an interrupted native installation.\n  uninstall                                     Remove owned integration and preserve profiles.\n  profiles --profiles-dir DIR                   Validate declarative mod profiles.\n  select --profiles-dir DIR --mod ID [--path ROOT] [--once]\n                                                Select eligible local/discovered Lua in place.\n  deselect --profiles-dir DIR --mod ID [--once]\n                                                Disable a future selection or cancel a queued launch.\n  diagnostics [--cache-dir DIR]                 Read the last in-game diagnostic report.\n\nGame commands accept --game-dir DIR or PZ_GAME_DIR.\nInstallation accepts --profiles-dir DIR, --debug, --no-debug, --early-ui, --no-early-ui.\n--once queues selection for the next game process, including reconnects, then expires.\nIt preserves mod source files and never hot-unloads running code.\nExplicit CLI command results are JSON; setup opens native desktop dialogs.\nWith no arguments, a Linux desktop session opens setup.\nGraphical setup requires Zenity or KDialog from the desktop.\nNo command downloads code or subscribes to Workshop items.\nLinux native launcher integration is the only verified installer platform.\nPackage integrity does not establish in-game runtime acceptance.\n";

struct Arguments {
    command: String,
    values: BTreeMap<String, String>,
    mods: Vec<String>,
    switches: BTreeSet<String>,
}

impl Arguments {
    fn parse() -> Result<Self, String> {
        let mut args = env::args().skip(1);
        let command = args.next().unwrap_or_else(|| "help".into());
        let mut result = Self {
            command,
            values: BTreeMap::new(),
            mods: Vec::new(),
            switches: BTreeSet::new(),
        };
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--game-dir" | "--package" | "--runtime" | "--profiles-dir" | "--out"
                | "--cache-dir" | "--artifact" | "--manifest" | "--sha256"
                | "--manifest-sha256" | "--provenance" | "--allow" | "--extension-id"
                | "--path" => {
                    let value = args
                        .next()
                        .ok_or_else(|| format!("{arg} requires a value"))?;
                    if result.values.insert(arg.clone(), value).is_some() {
                        return Err(format!("{arg} was supplied more than once"));
                    }
                }
                "--mod" => result
                    .mods
                    .push(args.next().ok_or("--mod requires a mod ID")?),
                "--debug"
                | "--no-debug"
                | "--early-ui"
                | "--no-early-ui"
                | "--json"
                | "--trust-native-code"
                | "--once" => {
                    result.switches.insert(arg);
                }
                "--help" | "-h" => {
                    result.command = "help".into();
                }
                _ => return Err(format!("Unknown argument {arg:?}. Run luabuddy help.")),
            }
        }
        if result.switches.contains("--debug") && result.switches.contains("--no-debug") {
            return Err("Choose either --debug or --no-debug".into());
        }
        if result.switches.contains("--early-ui") && result.switches.contains("--no-early-ui") {
            return Err("Choose either --early-ui or --no-early-ui".into());
        }
        Ok(result)
    }
    fn path(&self, name: &str) -> Option<PathBuf> {
        self.values.get(name).map(PathBuf::from)
    }
    fn required_path(&self, name: &str) -> Result<PathBuf, String> {
        self.path(name)
            .ok_or_else(|| format!("{} requires {name}", self.command))
    }
    fn bool_choice(&self, yes: &str, no: &str) -> Option<bool> {
        if self.switches.contains(yes) {
            Some(true)
        } else if self.switches.contains(no) {
            Some(false)
        } else {
            None
        }
    }
    fn allow(&self, values: &[&str], switches: &[&str], mods: bool) -> Result<(), String> {
        for value in self.values.keys() {
            if !values.contains(&value.as_str()) {
                return Err(format!("{} does not accept {value}", self.command));
            }
        }
        for switch in &self.switches {
            if switch != "--json" && !switches.contains(&switch.as_str()) {
                return Err(format!("{} does not accept {switch}", self.command));
            }
        }
        if !mods && !self.mods.is_empty() {
            return Err(format!("{} does not accept --mod", self.command));
        }
        Ok(())
    }
}

fn execute(args: &Arguments) -> Result<Value, String> {
    let game = || find_game(args.path("--game-dir").as_deref()).map_err(|e| e.to_string());
    let cache = || {
        args.path("--cache-dir")
            .or_else(|| {
                env::var_os("HOME")
                    .or_else(|| env::var_os("USERPROFILE"))
                    .map(|p| PathBuf::from(p).join("Zomboid"))
            })
            .ok_or_else(|| "Specify --cache-dir".to_owned())
    };
    match args.command.as_str() {
        "discover" => {
            args.allow(&["--game-dir"], &[], false)?;
            Ok(
                json!({"game":game()?, "steamLibraries":steam_libraries().map_err(|e| e.to_string())?}),
            )
        }
        "package" => {
            args.allow(&["--runtime", "--profiles-dir", "--out"], &[], false)?;
            let package = make_package(
                &args.required_path("--runtime")?,
                &args.required_path("--profiles-dir")?,
                &args.required_path("--out")?,
            )
            .map_err(|e| e.to_string())?;
            Ok(
                json!({"package":package.directory, "manifestSha256":package.digest, "manifest":package.manifest}),
            )
        }
        "verify-package" => {
            args.allow(&["--package"], &[], false)?;
            let package =
                verify_package(&args.required_path("--package")?).map_err(|e| e.to_string())?;
            Ok(
                json!({"ok":true, "package":package.directory, "manifestSha256":package.digest, "runtimeVerified":false}),
            )
        }
        "install" | "update" | "repair" => {
            args.allow(
                &["--game-dir", "--package", "--profiles-dir"],
                &["--debug", "--no-debug", "--early-ui", "--no-early-ui"],
                true,
            )?;
            install(
                &game()?,
                &InstallOptions {
                    package: args.required_path("--package")?,
                    profiles: args.path("--profiles-dir"),
                    mods: args.mods.clone(),
                    debug: args.bool_choice("--debug", "--no-debug"),
                    early_ui: args.bool_choice("--early-ui", "--no-early-ui"),
                    repair: args.command == "repair",
                },
            )
            .map_err(|e| e.to_string())
        }
        "verify" | "status" => {
            args.allow(&["--game-dir"], &[], false)?;
            verify_install(&game()?).map_err(|e| e.to_string())
        }
        "doctor" => {
            args.allow(&["--game-dir", "--cache-dir"], &[], false)?;
            doctor(&game()?, &cache()?).map_err(|e| e.to_string())
        }
        "rollback" | "recover" | "uninstall" => {
            args.allow(&["--game-dir"], &[], false)?;
            let game = game()?;
            match args.command.as_str() {
                "rollback" => rollback(&game),
                "recover" => recover_install(&game),
                _ => uninstall(&game),
            }
            .map_err(|e| e.to_string())
        }
        "select" | "deselect" => {
            args.allow(
                if args.command == "select" {
                    &["--profiles-dir", "--path"]
                } else {
                    &["--profiles-dir"]
                },
                &["--once"],
                true,
            )?;
            if args.mods.len() != 1 {
                return Err(format!("{} requires exactly one --mod ID", args.command));
            }
            let id = luabuddy_core::ModId::new(&args.mods[0]).map_err(|e| e.to_string())?;
            let directory = args.required_path("--profiles-dir")?;
            let once = args.switches.contains("--once");
            let lifetime = if once {
                luabuddy_core::selection::Lifetime::Once
            } else {
                luabuddy_core::selection::Lifetime::Persistent
            };
            if args.command == "select" {
                let profile = luabuddy_core::selection::select(
                    &directory,
                    &id,
                    args.path("--path").as_deref(),
                    lifetime,
                )
                .map_err(|e| e.to_string())?;
                Ok(
                    json!({"ok":true,"profile":profile,"lifetime":if once {"next-game-process"} else {"persistent"},"loaded":false,"sourcePreserved":true}),
                )
            } else {
                let changed = luabuddy_core::selection::deselect(&directory, &id, lifetime)
                    .map_err(|e| e.to_string())?;
                Ok(
                    json!({"ok":true,"id":id,"changed":changed,"lifetime":if once {"next-game-process"} else {"persistent"},"runningCodeUnloaded":false,"sourcePreserved":true}),
                )
            }
        }
        "profiles" => {
            args.allow(&["--profiles-dir"], &[], false)?;
            let directory = args.required_path("--profiles-dir")?;
            let profiles = luabuddy_core::read_profiles(&directory).map_err(|e| e.to_string())?;
            let queued =
                luabuddy_core::selection::pending(&directory).map_err(|e| e.to_string())?;
            Ok(json!({"ok":true, "profiles":profiles,"queuedForNextLaunch":queued}))
        }
        "diagnostics" => {
            args.allow(&["--cache-dir"], &[], false)?;
            diagnostics(&cache()?).map_err(|e| e.to_string())
        }
        "extension-install" | "extension-enable" => {
            let mut allowed = vec![
                "--profiles-dir",
                "--sha256",
                "--manifest-sha256",
                "--provenance",
                "--allow",
            ];
            if args.command == "extension-install" {
                allowed.extend(["--artifact", "--manifest"]);
            } else {
                allowed.push("--extension-id");
            }
            args.allow(&allowed, &["--trust-native-code"], false)?;
            let required = |key: &str| {
                args.values
                    .get(key)
                    .map(String::as_str)
                    .ok_or_else(|| format!("{} requires {key}", args.command))
            };
            let approval = luabuddy::extensions::Approval {
                sha256: required("--sha256")?,
                manifest_sha256: required("--manifest-sha256")?,
                provenance: required("--provenance")?,
                privileges: luabuddy::extensions::parse_privileges(required("--allow")?)
                    .map_err(|error| error.to_string())?,
                trust_native_code: args.switches.contains("--trust-native-code"),
            };
            let profiles = args.required_path("--profiles-dir")?;
            if args.command == "extension-install" {
                luabuddy::extensions::install(
                    &profiles,
                    &args.required_path("--artifact")?,
                    &args.required_path("--manifest")?,
                    approval,
                )
            } else {
                luabuddy::extensions::enable(&profiles, required("--extension-id")?, approval)
            }
            .map_err(|error| error.to_string())
        }
        "extension-disable" | "extensions" => {
            args.allow(
                if args.command == "extension-disable" {
                    &["--profiles-dir", "--extension-id"]
                } else {
                    &["--profiles-dir"]
                },
                &[],
                false,
            )?;
            let profiles = args.required_path("--profiles-dir")?;
            if args.command == "extension-disable" {
                let id = args
                    .values
                    .get("--extension-id")
                    .ok_or("extension-disable requires --extension-id")?;
                luabuddy::extensions::disable(&profiles, id)
            } else {
                luabuddy::extensions::list(&profiles)
            }
            .map_err(|error| error.to_string())
        }
        _ => Err(format!(
            "Unknown command {:?}. Run luabuddy help.",
            args.command
        )),
    }
}

fn main() {
    if env::args_os().len() == 1 && setup::graphical_session() {
        if let Err(error) = setup::run(setup::Options {
            package: None,
            game: None,
        }) {
            eprintln!("LuaBuddy setup: {error}");
            std::process::exit(2);
        }
        return;
    }
    let args = match Arguments::parse() {
        Ok(args) => args,
        Err(error) => {
            eprintln!("LuaBuddy: {error}");
            std::process::exit(2);
        }
    };
    if args.command == "setup" {
        let result = args
            .allow(&["--package", "--game-dir"], &[], false)
            .and_then(|()| {
                if args.switches.contains("--json") {
                    Err("setup is graphical and does not accept --json".into())
                } else {
                    setup::run(setup::Options {
                        package: args.path("--package"),
                        game: args.path("--game-dir"),
                    })
                }
            });
        if let Err(error) = result {
            eprintln!("LuaBuddy setup: {error}");
            std::process::exit(2);
        }
        return;
    }
    if matches!(args.command.as_str(), "help" | "--help" | "-h") {
        print!(
            "{HELP}\nNative extension commands require --profiles-dir DIR:\n  extension-install --artifact FILE --manifest FILE --sha256 SHA --manifest-sha256 MANIFEST_SHA --provenance SOURCE --allow PRIVILEGES --trust-native-code\n  extension-enable --extension-id ID --sha256 SHA --manifest-sha256 MANIFEST_SHA --provenance SOURCE --allow PRIVILEGES --trust-native-code\n  extension-disable --extension-id ID\n  extensions\nInstall stores native execution disabled. Enable and disable take effect in the next JVM process. Native extensions have full process privileges.\n"
        );
        return;
    }
    if matches!(args.command.as_str(), "version" | "--version" | "-V") {
        println!("LuaBuddy {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    match execute(&args) {
        Ok(value) => {
            match serde_json::to_string_pretty(&value) {
                Ok(encoded) => println!("{encoded}"),
                Err(error) => {
                    eprintln!("LuaBuddy: Encode result: {error}");
                    std::process::exit(2);
                }
            }
            if value.get("ok") == Some(&Value::Bool(false)) {
                std::process::exit(1);
            }
        }
        Err(error) => {
            eprintln!("LuaBuddy: {error}");
            std::process::exit(2);
        }
    }
}
