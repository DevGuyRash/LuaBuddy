mod desktop;
mod source;

use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use source::FrameSource;
use std::{
    collections::BTreeMap,
    env,
    ffi::OsString,
    fs::{self, File},
    io::{Read, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureCredentials {
    account: String,
    password: String,
}

fn fixture_credentials(path: &Path, expected: Option<&String>) -> Result<FixtureCredentials> {
    let metadata = fs::metadata(path)?;
    if metadata.permissions().mode() & 0o077 != 0 || metadata.len() > 8192 {
        return Err("Fixture credentials must be a private file of at most8192 bytes".into());
    }
    let value: FixtureCredentials = serde_json::from_slice(&fs::read(path)?)?;
    if expected != Some(&value.account)
        || value.password.is_empty()
        || value.password.len() > 128
        || value.password.chars().any(char::is_control)
    {
        return Err("Fixture credentials do not match the exact disposable account".into());
    }
    Ok(value)
}

const HELP: &str = "luabuddy-performance --game-dir DIR --runtime FILE --observer FILE --out DIR [--template-cache DIR] [--mode menu|world] [--rounds 5] [--warmup 10] [--seconds 30] [--deadline 300] [--single baseline|enabled] [--jvm-args JSONFILE] [--game-args JSONFILE] [--auto-ui] [--account NAME] [--fixture-credentials JSONFILE] [--prime-cache-out DIR] [--deciding] [--growth] [--native-reconnect-control] [--frame-source mesa|mango_hud] [--display DISPLAY --xauthority FILE] [--steam] [--native-favorite]\nA deciding suite uses five alternating runs per arm, at least 30 seconds measured per run, and an immutable release runtime. Native automation clicks only observed controls in its own client.\n";

struct ExistingDisplay {
    name: String,
    authority: PathBuf,
}

struct Options {
    game: PathBuf,
    runtime: PathBuf,
    observer: PathBuf,
    out: PathBuf,
    template: Option<PathBuf>,
    mode: String,
    rounds: usize,
    warmup: u64,
    seconds: u64,
    deadline: u64,
    single: Option<String>,
    jvm_args: Vec<String>,
    game_args: Vec<String>,
    deciding: bool,
    auto_ui: bool,
    prime: Option<PathBuf>,
    account: Option<String>,
    credentials: Option<FixtureCredentials>,
    growth: bool,
    native_control: bool,
    frame_source: FrameSource,
    existing_display: Option<ExistingDisplay>,
    steam: bool,
    native_favorite: bool,
}

impl Options {
    fn parse() -> Result<Self> {
        Self::parse_args(env::args().skip(1))
    }

    fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Self> {
        let mut values = BTreeMap::new();
        let mut deciding = false;
        let mut auto_ui = false;
        let mut growth = false;
        let mut native_control = false;
        let mut steam = false;
        let mut native_favorite = false;
        while let Some(key) = args.next() {
            if matches!(key.as_str(), "--help" | "-h") {
                print!("{HELP}");
                std::process::exit(0);
            }
            if key == "--deciding" {
                deciding = true;
                continue;
            }
            if key == "--auto-ui" {
                auto_ui = true;
                continue;
            }
            if key == "--growth" {
                growth = true;
                continue;
            }
            if key == "--native-reconnect-control" {
                native_control = true;
                growth = true;
                continue;
            }
            if key == "--steam" {
                steam = true;
                continue;
            }
            if key == "--native-favorite" {
                native_favorite = true;
                continue;
            }
            if !matches!(
                key.as_str(),
                "--game-dir"
                    | "--runtime"
                    | "--observer"
                    | "--out"
                    | "--template-cache"
                    | "--mode"
                    | "--rounds"
                    | "--warmup"
                    | "--seconds"
                    | "--deadline"
                    | "--single"
                    | "--jvm-args"
                    | "--game-args"
                    | "--prime-cache-out"
                    | "--account"
                    | "--fixture-credentials"
                    | "--frame-source"
                    | "--display"
                    | "--xauthority"
            ) {
                return Err(format!("Unknown argument {key}").into());
            }
            let value = args
                .next()
                .ok_or_else(|| format!("{key} requires a value"))?;
            if values.insert(key.clone(), value).is_some() {
                return Err(format!("Duplicate {key}").into());
            }
        }
        let path = |key: &str| -> Result<PathBuf> {
            Ok(
                PathBuf::from(values.get(key).ok_or_else(|| format!("Missing {key}"))?)
                    .canonicalize()?,
            )
        };
        let number = |key: &str, default: u64| -> Result<u64> {
            Ok(values
                .get(key)
                .map(|v| v.parse())
                .transpose()?
                .unwrap_or(default))
        };
        let extra = |key: &str| -> Result<Vec<String>> {
            Ok(match values.get(key) {
                Some(path) => serde_json::from_slice(&fs::read(path)?)?,
                None => Vec::new(),
            })
        };
        let out = PathBuf::from(values.get("--out").ok_or("Missing --out")?);
        fs::create_dir_all(&out)?;
        let existing_display = match (values.get("--display"), values.get("--xauthority")) {
            (Some(name), Some(_))
                if name.len() > 1
                    && name.starts_with(':')
                    && name[1..]
                        .chars()
                        .all(|value| value.is_ascii_digit() || value == '.') =>
            {
                Some(ExistingDisplay {
                    name: name.clone(),
                    authority: path("--xauthority")?,
                })
            }
            (None, None) => None,
            _ => {
                return Err(
                    "An existing local X11 display requires paired --display and --xauthority"
                        .into(),
                );
            }
        };
        let result = Self {
            game: path("--game-dir")?,
            runtime: path("--runtime")?,
            observer: path("--observer")?,
            out: out.canonicalize()?,
            template: values
                .get("--template-cache")
                .map(|p| PathBuf::from(p).canonicalize())
                .transpose()?,
            mode: values
                .get("--mode")
                .cloned()
                .unwrap_or_else(|| "menu".into()),
            rounds: number("--rounds", 5)? as usize,
            warmup: number("--warmup", 10)?,
            seconds: number("--seconds", 30)?,
            deadline: number("--deadline", 300)?,
            single: values.get("--single").cloned(),
            jvm_args: extra("--jvm-args")?,
            game_args: extra("--game-args")?,
            deciding,
            auto_ui,
            prime: values
                .get("--prime-cache-out")
                .map(std::path::absolute)
                .transpose()?,
            account: values.get("--account").cloned(),
            credentials: values
                .get("--fixture-credentials")
                .map(|path| fixture_credentials(Path::new(path), values.get("--account")))
                .transpose()?,
            growth,
            native_control,
            frame_source: match values
                .get("--frame-source")
                .map(String::as_str)
                .unwrap_or("mesa")
            {
                "mesa" => FrameSource::Mesa,
                "mango_hud" => FrameSource::MangoHud,
                _ => return Err("--frame-source must be mesa or mango_hud".into()),
            },
            existing_display,
            steam,
            native_favorite,
        };
        if result.frame_source == FrameSource::MangoHud && result.existing_display.is_none() {
            return Err(
                "The MangoHud workload requires an explicit existing hardware display".into(),
            );
        }
        if result.native_favorite
            && (!result.steam
                || result.mode != "world"
                || !result.auto_ui
                || result.template.is_none()
                || result.account.is_none())
        {
            return Err("Native Favorites startup requires Steam world mode, a primed private cache, UI automation, and an exact account".into());
        }
        if !matches!(result.mode.as_str(), "menu" | "world")
            || !(1..=20).contains(&result.rounds)
            || !(1..=120).contains(&result.warmup)
            || !(1..=300).contains(&result.seconds)
            || !(30..=7200).contains(&result.deadline)
        {
            return Err("Invalid workload bounds".into());
        }
        if result
            .single
            .as_ref()
            .is_some_and(|arm| !matches!(arm.as_str(), "baseline" | "enabled"))
        {
            return Err("--single must be baseline or enabled".into());
        }
        if result.jvm_args.iter().any(|value| {
            value.starts_with("-agent")
                || value.starts_with("-Dluabuddy.")
                || value.starts_with("-Duser.home=")
                || value.starts_with("-Dzomboid.steam=")
                || matches!(value.as_str(), "-cp" | "-classpath" | "-jar")
        }) {
            return Err("Extra JVM arguments cannot replace benchmark ownership, agents, or empty profile selection".into());
        }
        if result.deciding && (result.single.is_some() || result.rounds < 5 || result.seconds < 30)
        {
            return Err(
                "Deciding evidence needs five paired runs and at least 30 measured seconds".into(),
            );
        }
        if result.growth
            && (result.deciding
                || result.single.as_deref()
                    != Some(if result.native_control {
                        "baseline"
                    } else {
                        "enabled"
                    })
                || result.rounds != 1
                || result.mode != "world"
                || !result.auto_ui
                || result.template.is_none())
        {
            return Err("Lifecycle diagnostics need the matching single world arm, native UI automation, and the private primed fixture".into());
        }
        if result.prime.is_some()
            && (result.deciding || result.single.is_none() || result.rounds != 1)
        {
            return Err("A cache snapshot requires one non-deciding priming run".into());
        }
        if result.prime.as_ref().is_some_and(|path| path.exists()) {
            return Err("Refusing to overwrite an existing primed cache".into());
        }
        if !result.game.join("projectzomboid.jar").is_file()
            || !result.game.join("jre64/bin/java").is_file()
        {
            return Err("Expected the actual Linux game and bundled JVM".into());
        }
        if result.auto_ui
            && result.mode == "world"
            && result
                .account
                .as_ref()
                .is_none_or(|name| name.is_empty() || name.chars().any(char::is_control))
        {
            return Err(
                "World UI automation requires an exact disposable --account identity".into(),
            );
        }
        Ok(result)
    }
}

struct OwnedDirectory(PathBuf);

impl OwnedDirectory {
    fn new() -> Result<Self> {
        let parent = env::temp_dir().canonicalize()?;
        for serial in 0..100 {
            let path = parent.join(format!(
                "luabuddy-performance-{}-{}-{serial}",
                std::process::id(),
                SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
            ));
            match fs::create_dir(&path) {
                Ok(()) => {
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
                    fs::write(
                        path.join(".owned-by-performance"),
                        std::process::id().to_string(),
                    )?;
                    return Ok(Self(path));
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
                Err(error) => return Err(error.into()),
            }
        }
        Err("Cannot allocate private performance fixture".into())
    }
}

impl Drop for OwnedDirectory {
    fn drop(&mut self) {
        if fs::read_to_string(self.0.join(".owned-by-performance"))
            .is_ok_and(|value| value == std::process::id().to_string())
            && self.0.parent().is_some_and(|parent| {
                env::temp_dir()
                    .canonicalize()
                    .is_ok_and(|expected| parent == expected)
            })
            && let Err(error) = fs::remove_dir_all(&self.0)
        {
            eprintln!(
                "Owned performance cleanup failed {}: {error}",
                self.0.display()
            );
        }
    }
}

struct Process(Child);

impl Drop for Process {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

struct DisplayProcess {
    process: Process,
    number: u32,
}

impl Drop for DisplayProcess {
    fn drop(&mut self) {
        let pid = self.process.0.id();
        if matches!(self.process.0.try_wait(), Ok(None)) {
            let _ = Command::new("/usr/bin/kill")
                .args(["-TERM", &pid.to_string()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            let deadline = Instant::now() + Duration::from_secs(2);
            while matches!(self.process.0.try_wait(), Ok(None)) && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(20));
            }
        }
        if matches!(self.process.0.try_wait(), Ok(None)) {
            let _ = self.process.0.kill();
            let _ = self.process.0.wait();
        }
        let lock = PathBuf::from(format!("/tmp/.X{}-lock", self.number));
        if fs::read_to_string(&lock).is_ok_and(|value| value.trim() == pid.to_string())
            && !PathBuf::from(format!("/proc/{pid}")).exists()
        {
            let socket = PathBuf::from(format!("/tmp/.X11-unix/X{}", self.number));
            let _ = fs::remove_file(socket);
            let _ = fs::remove_file(lock);
        }
    }
}

fn hash(path: &Path) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(fs::read(path)?)))
}
fn write(path: &Path, value: &Value) -> Result<()> {
    fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}
fn wall_ms() -> Result<f64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs_f64() * 1000.0)
}

fn immutable(source: &Path, out: &Path, name: &str) -> Result<(PathBuf, String)> {
    fs::create_dir_all(out)?;
    let bytes = fs::read(source)?;
    let digest = format!("{:x}", Sha256::digest(&bytes));
    let path = out.join(format!("{name}-{digest}.so"));
    if path.exists() {
        if hash(&path)? != digest {
            return Err(format!("Existing immutable artifact changed: {}", path.display()).into());
        }
    } else {
        fs::write(&path, bytes)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o500))?;
    }
    Ok((path, digest))
}

fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        if matches!(
            name.to_str(),
            Some(
                "Logs"
                    | "logs.zip"
                    | "console.txt"
                    | "messaging"
                    | "Workshop"
                    | "mods"
                    | "LuaBuddy"
                    | "performance-recovery.json"
            )
        ) {
            continue;
        }
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            return Err(format!(
                "Fixture template contains a symlink: {}",
                entry.path().display()
            )
            .into());
        }
        if kind.is_dir() {
            copy_tree(&entry.path(), &destination.join(name))?;
        } else if kind.is_file() {
            fs::copy(entry.path(), destination.join(name))?;
        }
    }
    Ok(())
}

fn seed_cache(options: &Options, cache: &Path) -> Result<()> {
    if let Some(template) = &options.template {
        copy_tree(template, cache)?;
    } else {
        fs::create_dir_all(cache)?;
    }
    let path = cache.join("options.ini");
    let original = fs::read_to_string(&path).unwrap_or_default();
    let replacements = BTreeMap::from([
        ("width", "1280"),
        ("height", "800"),
        ("fullScreen", "false"),
        ("borderless", "false"),
        ("frameRate", "60"),
        ("populateServerListOnStart", "false"),
        ("focusloss", "false"),
        ("showSurvivalGuide", "false"),
    ]);
    let mut lines = original
        .lines()
        .filter(|line| {
            !line
                .split_once('=')
                .is_some_and(|(key, _)| replacements.contains_key(key))
        })
        .map(str::to_owned)
        .collect::<Vec<_>>();
    lines.extend(
        replacements
            .iter()
            .map(|(key, value)| format!("{key}={value}")),
    );
    fs::write(path, lines.join("\n") + "\n")?;
    fs::create_dir_all(cache.join("mods"))?;
    Ok(())
}

fn seed_growth(work: &Path) -> Result<()> {
    for (id, source, cleanup) in [
        (
            "LuaBuddyGrowth",
            "LuaBuddyGrowth = { ticks = 0 }; local current = LuaBuddyGrowth; Events.OnTick.Add(function() current.ticks = current.ticks + 1 end); function current.cleanup() return true end; return current\n",
            "cleanup=LuaBuddyGrowth.cleanup\n",
        ),
        (
            "LuaBuddyGrowthForeign",
            "LuaBuddyGrowthForeign = { ticks = 0 }; local current = LuaBuddyGrowthForeign; Events.OnTick.Add(function() current.ticks = current.ticks + 1 end); return current\n",
            "",
        ),
    ] {
        let root = work.join("mods").join(id).join("42");
        fs::create_dir_all(root.join("media/lua/shared"))?;
        fs::write(
            root.join("mod.info"),
            format!("name={id}\nid={id}\nversionMin=42.0\n"),
        )?;
        fs::write(
            root.join("media/lua/shared").join(format!("{id}.lua")),
            source,
        )?;
        fs::write(
            work.join("profiles").join(format!("{id}.properties")),
            format!("id={id}\npath=../mods/{id}\n{cleanup}"),
        )?;
    }
    Ok(())
}

fn native_cleanup(
    child: &mut Process,
    out: &Path,
    display: &str,
    authority: &Path,
    actions: &mut std::collections::BTreeSet<String>,
    credentials: Option<&FixtureCredentials>,
    desktop_guard: Option<&Path>,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(30);
    while child.0.try_wait()?.is_none() && Instant::now() < deadline {
        perform_control(
            &out.join("controls.json"),
            child.0.id(),
            display,
            authority,
            actions,
            credentials,
            desktop_guard,
        )?;
        thread::sleep(Duration::from_millis(200));
    }
    Ok(())
}

fn require_native_exit(child: &mut Process) -> Result<()> {
    match child.0.try_wait()? {
        Some(status) if status.success() => Ok(()),
        Some(status) => Err(format!("Owned native client closed unsuccessfully: {status}").into()),
        None => Err("Owned native client did not close normally within the cleanup bound".into()),
    }
}

fn client_errors(path: &Path) -> Result<Vec<String>> {
    Ok(fs::read_to_string(path)?
        .lines()
        .filter(|line| {
            line.contains("[LuaBuddy performance] FAIL")
                || line.contains("WARNING in native method")
                || line.contains("FATAL ERROR")
                || line.contains("[LuaBuddy] startup inactive")
                || line.contains("[LuaBuddy] startup failed:")
                || line.contains("[LuaBuddy] Runtime controls unavailable:")
        })
        .map(str::to_owned)
        .collect())
}

fn check_post_exit_log(out: &Path) -> Result<()> {
    let errors = client_errors(&out.join("client.log"))?;
    write(
        &out.join("final-log-check.json"),
        &json!({"checkedAfterNativeExit":true,"errorCount":errors.len(),"errors":errors}),
    )?;
    if !errors.is_empty() {
        return Err(format!(
            "Owned client logged {} final observer/JNI/runtime failures; see {}",
            errors.len(),
            out.join("final-log-check.json").display()
        )
        .into());
    }
    Ok(())
}

fn summarize_growth(value: &Value) -> Result<Value> {
    let records = value["records"]
        .as_array()
        .ok_or("Missing lifecycle records")?;
    let mut comparison = serde_json::Map::new();
    let native = value["nativeOnly"] == true;
    let kinds: &[(&str, usize)] = if native {
        &[("reconnect", 5)]
    } else {
        &[("refresh", 10), ("reload", 10), ("reconnect", 5)]
    };
    for &(kind, wanted) in kinds {
        let rows = records
            .iter()
            .filter(|r| r["kind"] == kind && r["warmup"] == false)
            .collect::<Vec<_>>();
        if rows.len() != wanted {
            return Err(format!("Incomplete {kind} lifecycle measurement").into());
        }
        let medians = rows
            .iter()
            .map(|r| {
                let values = r["rssBytes"]
                    .as_array()
                    .ok_or("Missing settled RSS samples")?
                    .iter()
                    .map(|v| v.as_f64().ok_or("Invalid RSS sample"))
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                percentile(&values, 0.5).ok_or("Empty settled RSS window")
            })
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let deltas = medians.windows(2).map(|v| v[1] - v[0]).collect::<Vec<_>>();
        let over = deltas
            .iter()
            .filter(|v| **v > 4.0 * 1024.0 * 1024.0)
            .count();
        let median_delta = percentile(&deltas, 0.5).ok_or("Missing settled RSS transitions")?;
        let net = medians.last().ok_or("Missing final RSS")?
            - medians.first().ok_or("Missing initial RSS")?;
        let net_per_cycle = net / deltas.len() as f64;
        let retained = rows
            .iter()
            .filter_map(|r| r["retiredWeakReferencesStillLive"].as_u64())
            .max()
            .unwrap_or(0);
        let resident = if (over >= 2 && net_per_cycle > 4.0 * 1024.0 * 1024.0)
            || median_delta > 4.0 * 1024.0 * 1024.0
        {
            "investigate repeatable resident growth"
        } else if net_per_cycle > 4.0 * 1024.0 * 1024.0 {
            "inconclusive net resident growth above budget; repeatability needs investigation"
        } else {
            "no repeatable resident growth observed in this window"
        };
        let reference = if retained == 0 {
            if native {
                "sampled retired native environments collected"
            } else {
                "sampled retired fixture objects collected"
            }
        } else if native {
            "sampled retired native environments remain live; investigation required"
        } else {
            "sampled retired fixture objects remain live; investigation required"
        };
        comparison.insert(kind.into(),json!({"measuredCycles":wanted,"discardedWarmupCycles":1,"rssMedianBytesByCycle":medians,"rssDeltaBytesByTransition":deltas,"medianRssDeltaBytes":median_delta,"netRssGrowthBytes":net,"netRssGrowthPerCycleBytes":net_per_cycle,"transitionsOverFourMiB":over,"maximumRetiredWeakReferencesStillLive":retained,"residentVerdict":resident,"referenceVerdict":reference,"verdict":if resident!="no repeatable resident growth observed in this window" {resident} else if retained>0 {"inconclusive retained weak references require investigation"} else {"sampled references bounded; no repeatable resident growth observed in this window"}}));
    }
    Ok(
        json!({"schema":1,"nativeOnly":native,"workload":if native {"native-only reconnect diagnostic control"} else {"lifecycle growth"},"complete":true,"comparison":comparison,"measurement":value,"limitation":"RSS alone cannot establish object retention; JVM and native renderer caches can retain committed memory. Weak references cover only the named objects, not all native allocations. The native-only control has no preceding Refresh/Reload history."}),
    )
}

fn display(work: &Path, out: &Path) -> Result<(DisplayProcess, String, PathBuf)> {
    let number = (177..277)
        .find(|number| !PathBuf::from(format!("/tmp/.X11-unix/X{number}")).exists())
        .ok_or("No private display number is available")?;
    let display = format!(":{number}");
    let authority = work.join("Xauthority");
    let mut cookie = [0_u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut cookie)?;
    let cookie = cookie
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    File::create(&authority)?;
    fs::set_permissions(&authority, fs::Permissions::from_mode(0o600))?;
    let status = Command::new("xauth")
        .arg("-f")
        .arg(&authority)
        .args(["add", &display, ".", &cookie])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if !status.success() {
        return Err("Cannot create private X display authorization".into());
    }
    let log = File::create(out.join("display.log"))?;
    let child = Command::new("Xvfb")
        .args([
            &display,
            "-screen",
            "0",
            "1280x800x24",
            "-nolisten",
            "tcp",
            "-auth",
        ])
        .arg(&authority)
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log))
        .spawn()?;
    let mut child = Process(child);
    let deadline = Instant::now() + Duration::from_secs(8);
    while !PathBuf::from(format!("/tmp/.X11-unix/X{number}")).exists() {
        if let Some(status) = child.0.try_wait()? {
            return Err(format!("Private Xvfb failed with {status}").into());
        }
        if Instant::now() > deadline {
            return Err("Private X display startup timed out".into());
        }
        thread::sleep(Duration::from_millis(50));
    }
    write(
        &out.join("owned-display.json"),
        &json!({"pid":child.0.id(),"display":display,"number":number,"authority":authority}),
    )?;
    Ok((
        DisplayProcess {
            process: child,
            number,
        },
        display,
        authority,
    ))
}

fn getconf(name: &str) -> Result<f64> {
    let result = Command::new("getconf").arg(name).output()?;
    if !result.status.success() {
        return Err(format!("getconf {name} failed").into());
    }
    Ok(std::str::from_utf8(&result.stdout)?.trim().parse()?)
}

fn process_sample(pid: u32, page: f64, ticks: f64) -> Result<Value> {
    let value = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let fields = value
        .rsplit_once(") ")
        .ok_or("Invalid proc process stat")?
        .1
        .split_whitespace()
        .collect::<Vec<_>>();
    if fields.len() < 22 {
        return Err("Truncated proc process stat".into());
    }
    let utime: f64 = fields[11].parse()?;
    let stime: f64 = fields[12].parse()?;
    let rss: f64 = fields[21].parse()?;
    Ok(
        json!({"unixMs":wall_ms()?,"cpuSeconds":(utime+stime)/ticks,"rssBytes":rss*page,"startTicks":fields[19],"threads":fs::read_to_string(format!("/proc/{pid}/status"))?.lines().find_map(|line|line.strip_prefix("Threads:")).unwrap_or("").trim()}),
    )
}

fn strings(value: &Value) -> Result<Vec<String>> {
    value
        .as_array()
        .ok_or("Expected a string array")?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| "Expected a string value".into())
        })
        .collect::<Result<Vec<_>>>()
}

fn percentile(values: &[f64], quantile: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let position = (sorted.len() - 1) as f64 * quantile;
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    Some(sorted[lower] + (sorted[upper] - sorted[lower]) * (position - lower as f64))
}

fn run(
    options: &Options,
    enabled: bool,
    index: usize,
    runtime: &Path,
    observer: &Path,
    page: f64,
    ticks: f64,
) -> Result<Value> {
    let arm = if enabled { "enabled" } else { "baseline" };
    let out = options.out.join(format!("{index:02}-{arm}"));
    fs::create_dir_all(&out)?;
    let desktop_guard = options
        .existing_display
        .as_ref()
        .map(|_| out.join("desktop-checks.jsonl"));
    desktop::require_unlocked(desktop_guard.as_deref(), "workload setup")?;
    let work = OwnedDirectory::new()?;
    let cache = work.0.join("cache");
    seed_cache(options, &cache)?;
    fs::create_dir_all(work.0.join("profiles"))?;
    if options.growth && !options.native_control {
        seed_growth(&work.0)?;
    }
    fs::create_dir_all(work.0.join("home"))?;
    let report = out.join("frames.json");
    if report.exists() {
        return Err(format!(
            "Refusing to overwrite existing deciding frame report {}",
            report.display()
        )
        .into());
    }
    fs::create_dir_all(out.join("hud"))?;
    let config = work.0.join("observer.json");
    write(
        &config,
        &json!({"schema":1,"output":report,"enabled":enabled,"mode":options.mode,"warmup_seconds":options.warmup,"measure_seconds":options.seconds,"progress":out.join("controls.json"),"account":options.account,"hud":if options.frame_source == FrameSource::Mesa {out.join("hud/frametime_(ms)")} else {out.join("hud")},"growth":options.growth,"cache":cache,"native_control":options.native_control,"frame_source":options.frame_source,"steam":options.steam,"native_favorite":options.native_favorite,"require_focus":options.existing_display.is_some()}),
    )?;
    let (_display_process, display, authority) = match &options.existing_display {
        Some(existing) => (None, existing.name.clone(), existing.authority.clone()),
        None => {
            let (process, name, authority) = display(&work.0, &out)?;
            (Some(process), name, authority)
        }
    };
    let base: Value =
        serde_json::from_slice(&fs::read(options.game.join("ProjectZomboid64.json"))?)?;
    let mut arguments = strings(&base["vmArgs"])?;
    arguments.retain(|value| {
        !value.starts_with("-javaagent:")
            && !value.starts_with("-agentpath:")
            && !value.starts_with("-agentlib:")
            && !value.starts_with("-Dluabuddy.")
            && !value.starts_with("-Dzomboid.steam=")
    });
    arguments.push(format!(
        "-agentpath:{}={}",
        observer.display(),
        config.display()
    ));
    if enabled {
        arguments.push(format!("-agentpath:{}", runtime.display()));
    }
    arguments.extend([
        format!("-Dzomboid.steam={}", if options.steam { 1 } else { 0 }),
        format!("-Duser.home={}", work.0.join("home").display()),
        format!("-Dluabuddy.config={}", work.0.join("profiles").display()),
    ]);
    arguments.extend(options.jvm_args.clone());
    arguments.extend([
        "-cp".into(),
        strings(&base["classpath"])?.join(":"),
        base["mainClass"]
            .as_str()
            .ok_or("Missing native main class")?
            .to_owned(),
        "-nosound".into(),
        "-novoip".into(),
        format!("-cachedir={}", cache.display()),
    ]);
    if !options.steam {
        arguments.push("-nosteam".into());
    }
    arguments.extend(options.game_args.clone());
    let log = File::create(out.join("client.log"))?;
    let mut path = OsString::from(options.game.join("jre64/bin"));
    path.push(":");
    path.push(env::var_os("PATH").unwrap_or_default());
    let mut libraries = OsString::from(options.game.join("natives"));
    libraries.push(":");
    libraries.push(env::var_os("LD_LIBRARY_PATH").unwrap_or_default());
    let mut command = if options.frame_source == FrameSource::MangoHud {
        let version = Command::new("/usr/bin/mangohud")
            .arg("--version")
            .output()?;
        if !version.status.success() || std::str::from_utf8(&version.stdout)?.trim() != "v0.8.4" {
            return Err("This CSV observer is verified against installed MangoHud v0.8.4".into());
        }
        let mango_config = work.0.join("MangoHud.conf");
        fs::write(
            &mango_config,
            format!(
                "alpha=0\nbackground_alpha=0\nautostart_log=1\nlog_interval=0\nlog_duration=0\npermit_upload=0\noutput_folder={}\n",
                out.join("hud").display()
            ),
        )?;
        let mut command = Command::new("/usr/bin/mangohud");
        command
            .arg("--dlsym")
            .arg(options.game.join("jre64/bin/java"))
            .env("MANGOHUD_CONFIGFILE", &mango_config)
            .env_remove("MANGOHUD_CONFIG")
            .env_remove("MANGOHUD_FPS_LIMIT")
            .env_remove("LIBGL_ALWAYS_SOFTWARE")
            .env_remove("GALLIUM_HUD")
            .env_remove("GALLIUM_HUD_PERIOD")
            .env_remove("GALLIUM_HUD_VISIBLE")
            .env_remove("GALLIUM_HUD_DUMP_DIR");
        fs::copy(&mango_config, out.join("MangoHud.conf"))?;
        command
    } else {
        let mut command = Command::new(options.game.join("jre64/bin/java"));
        command
            .env("LIBGL_ALWAYS_SOFTWARE", "1")
            .env("GALLIUM_HUD", "frametime")
            .env("GALLIUM_HUD_PERIOD", "0")
            .env("GALLIUM_HUD_VISIBLE", "false")
            .env("GALLIUM_HUD_DUMP_DIR", out.join("hud"));
        command
    };
    if options.steam {
        command
            .env("SteamAppId", "108600")
            .env("SteamGameId", "108600");
    }
    desktop::require_unlocked(desktop_guard.as_deref(), "game launch")?;
    let launched_wall = wall_ms()?;
    let mut child = Process(
        command
            .args(&arguments)
            .current_dir(&options.game)
            .env("DISPLAY", &display)
            .env("XAUTHORITY", &authority)
            .env("XDG_CACHE_HOME", work.0.join("xdg-cache"))
            .env("XDG_CONFIG_HOME", work.0.join("xdg-config"))
            .env("PATH", path)
            .env("LD_LIBRARY_PATH", libraries)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log))
            .spawn()?,
    );
    let marker = json!({"pid":child.0.id(),"display":display,"xauthority":authority,"cache":cache,"work":work.0,"enabled":enabled,"mode":options.mode,"launchedUnixMs":launched_wall,"report":report});
    write(&out.join("owned-client.json"), &marker)?;
    println!(
        "RUN {index} {arm} pid={} display={display} marker={}",
        child.0.id(),
        out.join("owned-client.json").display()
    );
    let deadline = Instant::now() + Duration::from_secs(options.deadline);
    let mut samples = Vec::new();
    let mut focused = false;
    let mut actions = std::collections::BTreeSet::new();
    while !report.is_file() {
        desktop::require_unlocked(desktop_guard.as_deref(), "workload sample")?;
        if let Some(status) = child.0.try_wait()? {
            return Err(format!(
                "Owned {arm} client exited before measurement: {status}; see {}",
                out.display()
            )
            .into());
        }
        if Instant::now() > deadline {
            return Err(format!(
                "Owned {arm} client failed to reach/finish usable workload before deadline; see {}",
                out.display()
            )
            .into());
        }
        samples.push(process_sample(child.0.id(), page, ticks)?);
        if options.existing_display.is_some() {
            let current = focused_owned_window(child.0.id(), &display, &authority);
            if focused && current.is_err() {
                return Err("Owned hardware workload lost focus; no refocus or further control was attempted".into());
            }
            focused |= current.is_ok();
        } else if !focused
            && let Ok(result) = Command::new("xdotool")
                .args([
                    "search",
                    "--onlyvisible",
                    "--pid",
                    &child.0.id().to_string(),
                    "--name",
                    "^Project Zomboid$",
                ])
                .env("DISPLAY", &display)
                .env("XAUTHORITY", &authority)
                .output()
            && result.status.success()
            && let Some(window) = std::str::from_utf8(&result.stdout)?.lines().next()
        {
            focused = Command::new("xdotool")
                .args(["windowfocus", window])
                .env("DISPLAY", &display)
                .env("XAUTHORITY", &authority)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()?
                .success();
        }
        if options.auto_ui && focused {
            perform_control(
                &out.join("controls.json"),
                child.0.id(),
                &display,
                &authority,
                &mut actions,
                options.credentials.as_ref(),
                desktop_guard.as_deref(),
            )?;
        }
        thread::sleep(Duration::from_millis(250));
    }
    desktop::require_unlocked(desktop_guard.as_deref(), "final workload sample")?;
    samples.push(process_sample(child.0.id(), page, ticks)?);
    desktop::require_unlocked(desktop_guard.as_deref(), "workload capture")?;
    let frames: Value = serde_json::from_slice(&fs::read(&report)?)?;
    write(&out.join("process-samples.json"), &json!(samples))?;
    let image = out.join("usable-workload.png");
    let capture = if options.existing_display.is_some() {
        focused_owned_window(child.0.id(), &display, &authority)?
    } else {
        "root".into()
    };
    let _ = Command::new("import")
        .args(["-window", &capture])
        .arg(&image)
        .env("DISPLAY", &display)
        .env("XAUTHORITY", &authority)
        .status();
    let maps = fs::read_to_string(format!("/proc/{}/maps", child.0.id()))?;
    let mapped = |name: &str| maps.lines().any(|line| line.contains(name));
    let library_evidence = json!({"pid":child.0.id(),"runtimeMapped":mapped(&runtime.to_string_lossy()),"mangoHudMapped":mapped("libMangoHud_opengl.so"),"nvidiaMapped":mapped("libnvidia-glcore.so"),"steamApiMapped":mapped("libsteam_api.so"),"steamJniMapped":mapped("libZNetJNI64.so")});
    write(&out.join("mapped-libraries.json"), &library_evidence)?;
    if !frames["error"].is_null()
        || !options.growth && frames["frame_count"].as_u64().unwrap_or(0) < 2
    {
        if options.auto_ui {
            native_cleanup(
                &mut child,
                &out,
                &display,
                &authority,
                &mut actions,
                options.credentials.as_ref(),
                desktop_guard.as_deref(),
            )?;
            write(
                &out.join("cleanup.json"),
                &json!({"nativeGracefulExit":child.0.try_wait()?.is_some_and(|s|s.success()),"measurementRejected":true,"performedNativeControls":actions}),
            )?;
        }
        let reason = frames["error"]
            .as_str()
            .unwrap_or("Insufficient completed frame work");
        return Err(format!(
            "Performance observer did not complete valid work: {reason}; see {}",
            report.display()
        )
        .into());
    }
    let native_steam_confirmed = if options.growth {
        frames["records"].as_array().is_some_and(|records| {
            !records.is_empty()
                && records
                    .iter()
                    .all(|record| record["nativeState"]["steam_enabled"] == true)
        })
    } else {
        frames["ready"]["steam_enabled"] == true && frames["end_ready"]["steam_enabled"] == true
    };
    if library_evidence["runtimeMapped"] != enabled
        || options.frame_source == FrameSource::MangoHud
            && (library_evidence["mangoHudMapped"] != true
                || library_evidence["nvidiaMapped"] != true)
        || options.steam
            && (library_evidence["steamApiMapped"] != true
                || library_evidence["steamJniMapped"] != true
                || !native_steam_confirmed)
        || options.steam
            && !fs::read_to_string(out.join("client.log"))?
                .contains("SteamUtils initialised successfully")
    {
        return Err(
            "Owned workload library maps or native Steam state do not match the declared arm"
                .into(),
        );
    }
    let errors = client_errors(&out.join("client.log"))?;
    if options.growth {
        if !errors.is_empty() {
            return Err(
                format!("Growth client logged observer/JNI/runtime failures: {errors:?}").into(),
            );
        }
        let result = summarize_growth(&frames)?;
        write(&out.join("result.json"), &result)?;
        native_cleanup(
            &mut child,
            &out,
            &display,
            &authority,
            &mut actions,
            options.credentials.as_ref(),
            desktop_guard.as_deref(),
        )?;
        write(
            &out.join("cleanup.json"),
            &json!({"nativeGracefulExit":child.0.try_wait()?.is_some_and(|s|s.success()),"performedNativeControls":actions}),
        )?;
        require_native_exit(&mut child)?;
        check_post_exit_log(&out)?;
        return Ok(result);
    }
    let target = frames["measure_start_unix_ms"]
        .as_f64()
        .ok_or("Missing exact measure-start clock")?;
    let settled = samples
        .iter()
        .filter(|sample| sample["unixMs"].as_f64().is_some_and(|time| time >= target))
        .collect::<Vec<_>>();
    let first = settled.first().ok_or("No measured process samples")?;
    let last = settled.last().ok_or("No final process sample")?;
    let cpu = last["cpuSeconds"]
        .as_f64()
        .ok_or("Invalid process CPU count")?
        - first["cpuSeconds"]
            .as_f64()
            .ok_or("Invalid process CPU count")?;
    let rss = settled
        .iter()
        .filter_map(|sample| sample["rssBytes"].as_f64())
        .collect::<Vec<_>>();
    let timings = frames["frame_ms"]
        .as_array()
        .ok_or("Missing actual presentation intervals")?
        .iter()
        .map(|value| value.as_f64().ok_or("Invalid frame interval"))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let record = json!({"schema":1,"arm":arm,"index":index,"ok":errors.is_empty(),"errors":errors,"startupMs":frames["ready_unix_ms"].as_f64().ok_or("Missing readiness clock")?-launched_wall,"observerStartupMs":frames["startup_ms"],"frameCount":timings.len(),"p50Ms":percentile(&timings,0.5),"p95Ms":percentile(&timings,0.95),"p99Ms":percentile(&timings,0.99),"cpuSeconds":cpu,"cpuWindowSeconds":(last["unixMs"].as_f64().ok_or("Missing process clock")?-first["unixMs"].as_f64().ok_or("Missing process clock")?)/1000.0,"rssMedianBytes":percentile(&rss,0.5),"rssMaxBytes":rss.iter().copied().max_by(f64::total_cmp),"measurement":frames,"machineLoad":fs::read_to_string("/proc/loadavg")?,"focused":focused});
    write(&out.join("result.json"), &record)?;
    println!(
        "COMPLETE {index} {arm} frames={} p95Ms={} startupMs={}",
        record["frameCount"], record["p95Ms"], record["startupMs"]
    );
    if !errors.is_empty() {
        return Err(format!("Owned {arm} client logged observer/JNI failures").into());
    }
    if options.auto_ui {
        native_cleanup(
            &mut child,
            &out,
            &display,
            &authority,
            &mut actions,
            options.credentials.as_ref(),
            desktop_guard.as_deref(),
        )?;
    }
    let graceful = child.0.try_wait()?.is_some_and(|status| status.success());
    write(
        &out.join("cleanup.json"),
        &json!({"nativeGracefulExit":graceful,"primedCache":options.prime,"performedNativeControls":actions}),
    )?;
    if let Some(destination) = &options.prime
        && !graceful
    {
        let _ = child.0.kill();
        let _ = child.0.wait();
        let recovery = destination.with_extension("unvalidated");
        if !recovery.exists() {
            fs::create_dir(&recovery)?;
            fs::set_permissions(&recovery, fs::Permissions::from_mode(0o700))?;
            copy_tree(&cache, &recovery)?;
            write(
                &recovery.join("performance-recovery.json"),
                &json!({"schema":1,"validated":false,"reason":"Native client did not close normally; not deciding evidence"}),
            )?;
        }
        return Err(format!(
            "Priming did not close normally; unvalidated recovery retained at {}",
            recovery.display()
        )
        .into());
    }
    require_native_exit(&mut child)?;
    check_post_exit_log(&out)?;
    if let Some(destination) = &options.prime {
        fs::create_dir(destination)?;
        fs::set_permissions(destination, fs::Permissions::from_mode(0o700))?;
        copy_tree(&cache, destination)?;
    }
    Ok(record)
}

fn focused_owned_window(pid: u32, display: &str, authority: &Path) -> Result<String> {
    let focus = Command::new("xdotool")
        .arg("getwindowfocus")
        .env("DISPLAY", display)
        .env("XAUTHORITY", authority)
        .output()?;
    if !focus.status.success() {
        return Err("Cannot inspect native window focus".into());
    }
    let window = std::str::from_utf8(&focus.stdout)?
        .trim()
        .parse::<u64>()?
        .to_string();
    let owner = Command::new("xdotool")
        .args(["getwindowpid", &window])
        .env("DISPLAY", display)
        .env("XAUTHORITY", authority)
        .output()?;
    if !owner.status.success() || std::str::from_utf8(&owner.stdout)?.trim().parse::<u32>()? != pid
    {
        return Err("Native window focus does not belong to the exact owned client".into());
    }
    Ok(window)
}

fn key(
    pid: u32,
    display: &str,
    authority: &Path,
    value: &str,
    desktop_guard: Option<&Path>,
) -> Result<()> {
    desktop::require_unlocked(desktop_guard, "native key press")?;
    let window = focused_owned_window(pid, display, authority)?;
    if !Command::new("xdotool")
        .args(["keydown", value])
        .env("DISPLAY", display)
        .env("XAUTHORITY", authority)
        .status()?
        .success()
    {
        return Err("Owned native key-down failed".into());
    }
    thread::sleep(Duration::from_millis(300));
    if !Command::new("xdotool")
        .args(["keyup", value])
        .env("DISPLAY", display)
        .env("XAUTHORITY", authority)
        .status()?
        .success()
    {
        return Err("Owned native key-up failed".into());
    }
    if focused_owned_window(pid, display, authority)? != window {
        return Err("Owned native key lost its target focus".into());
    }
    Ok(())
}

fn perform_control(
    path: &Path,
    pid: u32,
    display: &str,
    authority: &Path,
    done: &mut std::collections::BTreeSet<String>,
    credentials: Option<&FixtureCredentials>,
    desktop_guard: Option<&Path>,
) -> Result<()> {
    let Some(bytes) = fs::read(path).ok() else {
        return Ok(());
    };
    let Some(value) = serde_json::from_slice::<Value>(&bytes)
        .ok()
        .filter(|value| !value.is_null())
    else {
        return Ok(());
    };
    if value["pid"].as_u64() != Some(u64::from(pid)) {
        return Err("Native control belongs to another process".into());
    }
    let id = value["id"]
        .as_str()
        .ok_or("Native control has no identity")?;
    let sequence = value["sequence"].as_u64().unwrap_or(0);
    let name = value["name"].as_str().unwrap_or("");
    let identity = format!("{id}/{name}/{sequence}");
    if done.contains(&identity)
        && !matches!(
            id,
            "MainScreen.open-pause-menu"
                | "Growth.open-pause"
                | "MainScreen.nativeQuitLabel"
                | "SurvivalGuide.closeButton"
                | "MainOptions.nativeBack"
                | "MainOptions.nativeModalDismiss"
                | "ServerConnectPopup.instance.connectBtn"
                | "ServerConnectPopup.cancelBtn"
                | "ConnectToServer.nativeBack"
                | "MultiplayerUI.nativeBack"
                | "GameLoadingState.click-to-start"
        )
        && !(id == "Growth.native-button"
            && matches!(
                name,
                "open-options"
                    | "mods-scroll-left"
                    | "mods-scroll-right"
                    | "mods-tab"
                    | "dismiss-result"
                    | "options-back"
                    | "resume"
                    | "leave-world"
                    | "multiplayer"
                    | "reconnect-account"
            ))
    {
        return Ok(());
    }
    desktop::require_unlocked(desktop_guard, "native UI action")?;
    match (value["action"].as_str(), id) {
        (Some("fixture-login"), "ServerConnectPopup.fixture-credentials") => {
            let credentials = credentials.ok_or(
                "The native popup needs the explicitly supplied disposable fixture credentials",
            )?;
            input_fixture(
                pid,
                display,
                authority,
                &value["password"],
                &credentials.password,
                desktop_guard,
            )?;
            input_fixture(
                pid,
                display,
                authority,
                &value["username"],
                &credentials.account,
                desktop_guard,
            )?;
        }
        (
            Some("click"),
            "ServerConnectPopup.instance.connectBtn"
            | "ServerConnectPopup.cancelBtn"
            | "ConnectToServer.nativeBack"
            | "MultiplayerUI.nativeBack"
            | "GameLoadingState.click-to-start"
            | "SurvivalGuide.closeButton"
            | "MainScreen.nativeQuitLabel"
            | "MainScreen.quitToDesktopDialog.yes"
            | "MainOptions.nativeBack"
            | "MainOptions.nativeModalDismiss"
            | "Growth.native-button",
        ) => {
            let x = value["x"]
                .as_f64()
                .filter(|v| v.is_finite() && (0.0..1280.0).contains(v))
                .ok_or("Invalid native control X")?;
            let y = value["y"]
                .as_f64()
                .filter(|v| v.is_finite() && (0.0..800.0).contains(v))
                .ok_or("Invalid native control Y")?;
            let window = focused_owned_window(pid, display, authority)?;
            if !Command::new("xdotool")
                .args([
                    "mousemove",
                    "--window",
                    &window,
                    &format!("{x:.0}"),
                    &format!("{y:.0}"),
                ])
                .env("DISPLAY", display)
                .env("XAUTHORITY", authority)
                .status()?
                .success()
            {
                return Err("Owned native control positioning failed".into());
            }
            thread::sleep(Duration::from_millis(150));
            desktop::require_unlocked(desktop_guard, "native pointer press")?;
            if focused_owned_window(pid, display, authority)? != window {
                return Err("Owned native click lost focus before press".into());
            }
            if !Command::new("xdotool")
                .args(["mousedown", "1"])
                .env("DISPLAY", display)
                .env("XAUTHORITY", authority)
                .status()?
                .success()
            {
                return Err("Owned native click-down failed".into());
            }
            thread::sleep(Duration::from_millis(650));
            if !Command::new("xdotool")
                .args(["mouseup", "1"])
                .env("DISPLAY", display)
                .env("XAUTHORITY", authority)
                .status()?
                .success()
            {
                return Err("Owned native click-up failed".into());
            }
            if focused_owned_window(pid, display, authority)? != window {
                return Err("Owned native click lost focus during press".into());
            }
        }
        (Some("key"), "MainScreen.open-pause-menu") => {
            key(pid, display, authority, "Escape", desktop_guard)?;
            thread::sleep(Duration::from_secs(1));
        }
        (Some("key"), "Growth.open-pause") => {
            key(pid, display, authority, "Escape", desktop_guard)?;
            thread::sleep(Duration::from_secs(1));
        }
        (Some("profile-input"), "Growth.profile-id") => input_fixture(
            pid,
            display,
            authority,
            &value,
            "LuaBuddyGrowth",
            desktop_guard,
        )?,
        _ => return Err("Unrecognized native verification control".into()),
    }
    done.insert(identity);
    println!("NATIVE CONTROL {id} {name} {sequence}");
    Ok(())
}

fn input_fixture(
    pid: u32,
    display: &str,
    authority: &Path,
    field: &Value,
    text: &str,
    desktop_guard: Option<&Path>,
) -> Result<()> {
    desktop::require_unlocked(desktop_guard, "fixture field selection")?;
    let window = focused_owned_window(pid, display, authority)?;
    let x = field["x"]
        .as_f64()
        .filter(|value| value.is_finite() && (0.0..1280.0).contains(value))
        .ok_or("Invalid native fixture input X")?;
    let y = field["y"]
        .as_f64()
        .filter(|value| value.is_finite() && (0.0..800.0).contains(value))
        .ok_or("Invalid native fixture input Y")?;
    for args in [
        vec![
            "mousemove".to_owned(),
            "--window".into(),
            window.clone(),
            format!("{x:.0}"),
            format!("{y:.0}"),
            "mousedown".into(),
            "1".into(),
        ],
        vec!["mouseup".into(), "1".into()],
    ] {
        if !Command::new("xdotool")
            .args(args)
            .env("DISPLAY", display)
            .env("XAUTHORITY", authority)
            .status()?
            .success()
        {
            return Err("Owned fixture input focus failed".into());
        }
        thread::sleep(Duration::from_millis(400));
    }
    key(pid, display, authority, "ctrl+a", desktop_guard)?;
    if focused_owned_window(pid, display, authority)? != window {
        return Err("Owned fixture input lost focus".into());
    }
    desktop::require_unlocked(desktop_guard, "targeted fixture typing")?;
    let mut typing = Process(
        Command::new("xdotool")
            .args([
                "type",
                "--window",
                &window,
                "--clearmodifiers",
                "--delay",
                "80",
                "--file",
                "-",
            ])
            .env("DISPLAY", display)
            .env("XAUTHORITY", authority)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?,
    );
    let mut input = typing
        .0
        .stdin
        .take()
        .ok_or("Fixture typing has no private stdin")?;
    input.write_all(text.as_bytes())?;
    drop(input);
    if !typing.0.wait()?.success() {
        return Err(
            "Typing explicitly supplied fixture data into the owned native field failed".into(),
        );
    }
    if focused_owned_window(pid, display, authority)? != window {
        return Err("Owned fixture input lost focus during targeted typing".into());
    }
    desktop::require_unlocked(desktop_guard, "fixture typing completion")?;
    thread::sleep(Duration::from_millis(300));
    Ok(())
}

fn summarize(records: &[Value], deciding: bool, expected: usize) -> Value {
    let complete = records.len() == expected;
    let mut comparison = serde_json::Map::new();
    let mut overall = "within budget on observed workload";
    for (key, limit) in [
        ("startupMs", 0.05),
        ("p95Ms", 0.05),
        ("p99Ms", 0.10),
        ("rssMedianBytes", 0.0),
        ("cpuSeconds", 0.0),
    ] {
        let values = |arm: &str| {
            records
                .iter()
                .filter(|value| value["arm"] == arm)
                .filter_map(|value| value[key].as_f64())
                .collect::<Vec<_>>()
        };
        let baseline = values("baseline");
        let enabled = values("enabled");
        let (Some(a), Some(b)) = (percentile(&baseline, 0.5), percentile(&enabled, 0.5)) else {
            continue;
        };
        let spread = |data: &[f64]| {
            data.iter().copied().max_by(f64::total_cmp).unwrap_or(0.0)
                - data.iter().copied().min_by(f64::total_cmp).unwrap_or(0.0)
        };
        let delta = b - a;
        let budget = if key == "rssMedianBytes" {
            64.0 * 1024.0 * 1024.0
        } else {
            a * limit
        };
        let verdict = if !deciding || !complete {
            "pilot only"
        } else if key == "cpuSeconds" {
            "context only"
        } else if delta > budget && delta.abs() > spread(&baseline).max(spread(&enabled)) {
            overall = "budget exceeded";
            "budget exceeded"
        } else if delta.abs() <= spread(&baseline).max(spread(&enabled)) {
            if overall != "budget exceeded" {
                overall = "inconclusive under observed variation";
            }
            "inconclusive under observed variation"
        } else {
            "within budget on observed workload"
        };
        comparison.insert(key.into(), json!({"baselineMedian":a,"enabledMedian":b,"delta":delta,"relativeDelta":delta/a,"baselineRange":[baseline.iter().copied().min_by(f64::total_cmp),baseline.iter().copied().max_by(f64::total_cmp)],"enabledRange":[enabled.iter().copied().min_by(f64::total_cmp),enabled.iter().copied().max_by(f64::total_cmp)],"verdict":verdict}));
    }
    json!({"schema":1,"deciding":deciding && complete,"complete":complete,"expectedRuns":expected,"verdict":if !complete { "incomplete suite" } else if deciding { overall } else { "pilot only" },"comparison":comparison,"runs":records,"growth":"not measured by this presentation workload; requires repeated profile/reconnect scenario"})
}

fn main() -> Result<()> {
    let options = Options::parse()?;
    if options.out.join("workload.json").exists() {
        return Err(
            "Use a new evidence directory; an existing workload must not be overwritten".into(),
        );
    }
    let (runtime, runtime_hash) =
        immutable(&options.runtime, &options.out.join("artifacts"), "runtime")?;
    let (observer, observer_hash) = immutable(
        &options.observer,
        &options.out.join("artifacts"),
        "observer",
    )?;
    let mango_artifacts = if options.frame_source == FrameSource::MangoHud {
        let mut files = serde_json::Map::new();
        for path in [
            "/usr/bin/mangohud",
            "/usr/lib/mangohud/libMangoHud_shim.so",
            "/usr/lib/mangohud/libMangoHud_opengl.so",
        ] {
            files.insert(path.into(), json!(hash(Path::new(path))?));
        }
        Some(files)
    } else {
        None
    };
    let settings = json!({"schema":1,"gameJarSha256":hash(&options.game.join("projectzomboid.jar"))?,"runtimeSha256":runtime_hash,"observerSha256":observer_hash,"driverSha256":hash(&env::current_exe()?)?,"growth":options.growth,"nativeControl":options.native_control,"mode":options.mode,"roundsPerArm":options.rounds,"warmupSeconds":options.warmup,"measurementSeconds":options.seconds,"deciding":options.deciding,"cpuCount":std::thread::available_parallelism()?.get(),"initialLoad":fs::read_to_string("/proc/loadavg")?,"jvmRelease":fs::read_to_string(options.game.join("jre64/release"))?,"frameSource":options.frame_source,"presentationTarget":options.frame_source.target(),"presentationObserver":options.frame_source.description(),"mangoHudArtifacts":mango_artifacts,"existingDisplay":options.existing_display.as_ref().map(|value| &value.name),"steam":options.steam,"nativeFavorite":options.native_favorite,"observer":"identical presentation observer and read-only JVMTI agent thread in both arms; no Java code edits, breakpoints, or global tracing"});
    write(&options.out.join("workload.json"), &settings)?;
    let page = getconf("PAGESIZE")?;
    let ticks = getconf("CLK_TCK")?;
    let mut records = Vec::new();
    let arms = options
        .single
        .as_ref()
        .map(|arm| vec![arm == "enabled"])
        .unwrap_or_else(|| vec![false, true]);
    for round in 0..options.rounds {
        for enabled in &arms {
            let index = records.len() + 1;
            match run(&options, *enabled, index, &runtime, &observer, page, ticks) {
                Ok(record) => {
                    records.push(record);
                    write(
                        &options.out.join("summary.json"),
                        &if options.growth {
                            records[0].clone()
                        } else {
                            summarize(&records, options.deciding, options.rounds * arms.len())
                        },
                    )?;
                }
                Err(error) => {
                    write(
                        &options.out.join("failure.json"),
                        &json!({"schema":1,"round":round+1,"enabled":enabled,"error":error.to_string(),"completedRuns":records.len()}),
                    )?;
                    return Err(error);
                }
            }
        }
    }
    let summary = if options.growth {
        records[0].clone()
    } else {
        summarize(&records, options.deciding, options.rounds * arms.len())
    };
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"verdict":summary["verdict"],"comparison":summary["comparison"],"completedRuns":records.len(),"evidence":options.out})
        )?
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deciding_inputs_accept_packaged_and_resolved_cache_paths_and_keep_workload_gates() {
        let fixture = OwnedDirectory::new().unwrap();
        let game = fixture.0.join("game");
        let packaged = fixture.0.join("dist/LuaBuddy-linux-x86_64/runtime.so");
        let cached = fixture.0.join("cache/runtime.so");
        let observer = fixture.0.join("observer.so");
        for path in [
            game.join("projectzomboid.jar"),
            game.join("jre64/bin/java"),
            packaged.clone(),
            cached.clone(),
            observer.clone(),
        ] {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"argument validation fixture").unwrap();
        }
        let args = |runtime: &Path| {
            vec![
                "--game-dir".to_owned(),
                game.to_string_lossy().into_owned(),
                "--runtime".into(),
                runtime.to_string_lossy().into_owned(),
                "--observer".into(),
                observer.to_string_lossy().into_owned(),
                "--out".into(),
                fixture.0.join("out").to_string_lossy().into_owned(),
                "--deciding".into(),
            ]
        };
        let options = Options::parse_args(args(&packaged).into_iter()).unwrap();
        assert!(options.deciding);
        assert_eq!(options.runtime, packaged.canonicalize().unwrap());
        assert_eq!((options.rounds, options.seconds), (5, 30));

        let release_link = fixture.0.join("release/runtime.so");
        fs::create_dir(release_link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&cached, &release_link).unwrap();
        let options = Options::parse_args(args(&release_link).into_iter()).unwrap();
        assert_eq!(options.runtime, cached.canonicalize().unwrap());

        for (key, value) in [
            ("--rounds", "4"),
            ("--seconds", "29"),
            ("--single", "baseline"),
        ] {
            let mut arguments = args(&packaged);
            arguments.extend([key.to_owned(), value.to_owned()]);
            assert!(Options::parse_args(arguments.into_iter()).is_err());
        }
    }

    #[test]
    fn a_late_cleanup_fault_is_rechecked_from_the_complete_log() {
        let fixture = OwnedDirectory::new().unwrap();
        fs::write(fixture.0.join("client.log"), "Native world completed\n").unwrap();
        assert!(check_post_exit_log(&fixture.0).is_ok());
        fs::write(
            fixture.0.join("client.log"),
            "Native world completed\nWARNING in native method: late cleanup fault\n",
        )
        .unwrap();
        assert!(check_post_exit_log(&fixture.0).is_err());
        let value: Value =
            serde_json::from_slice(&fs::read(fixture.0.join("final-log-check.json")).unwrap())
                .unwrap();
        assert_eq!(value["errorCount"], 1);
        assert_eq!(value["checkedAfterNativeExit"], true);
    }

    #[test]
    fn native_only_control_summarizes_five_measured_reconnects_without_fixture_operations() {
        let mut records=(0..6).map(|cycle|json!({"kind":"reconnect","cycle":cycle,"warmup":cycle==0,"rssBytes":[(300+cycle*2)*1024*1024],"retiredWeakReferencesStillLive":0})).collect::<Vec<_>>();
        let value = summarize_growth(&json!({"nativeOnly":true,"records":records})).unwrap();
        assert_eq!(value["nativeOnly"], true);
        assert_eq!(value["comparison"].as_object().unwrap().len(), 1);
        assert_eq!(value["comparison"]["reconnect"]["measuredCycles"], 5);
        assert_eq!(
            value["comparison"]["reconnect"]["netRssGrowthPerCycleBytes"],
            2.0 * 1024.0 * 1024.0
        );
        records.pop();
        assert!(summarize_growth(&json!({"nativeOnly":true,"records":records})).is_err());
    }

    #[test]
    fn nonzero_or_live_native_process_cannot_be_accepted_as_normal_cleanup() {
        let mut failed = Process(Command::new("sh").args(["-c", "exit 7"]).spawn().unwrap());
        failed.0.wait().unwrap();
        assert!(require_native_exit(&mut failed).is_err());
        let mut live = Process(Command::new("sleep").arg("30").spawn().unwrap());
        assert!(require_native_exit(&mut live).is_err());
        drop(live);
        let mut normal = Process(Command::new("true").spawn().unwrap());
        normal.0.wait().unwrap();
        assert!(require_native_exit(&mut normal).is_ok());
    }

    #[test]
    fn periodic_eighteen_mib_increases_every_third_cycle_require_investigation() {
        let mut records = Vec::new();
        for (kind, count) in [("refresh", 10), ("reload", 10), ("reconnect", 5)] {
            for cycle in 0..count {
                let bytes = 100_u64 * 1024 * 1024
                    + if kind == "reload" {
                        (cycle / 3) * 18 * 1024 * 1024
                    } else {
                        0
                    };
                records.push(json!({"kind":kind,"cycle":cycle,"warmup":false,"rssBytes":[bytes,bytes,bytes],"retiredWeakReferencesStillLive":0}));
            }
        }
        let report = summarize_growth(&json!({"records":records})).unwrap();
        let reload = &report["comparison"]["reload"];
        assert_eq!(reload["medianRssDeltaBytes"], 0.0);
        assert_eq!(reload["netRssGrowthPerCycleBytes"], 6.0 * 1024.0 * 1024.0);
        assert_eq!(reload["transitionsOverFourMiB"], 3);
        assert_eq!(
            reload["residentVerdict"],
            "investigate repeatable resident growth"
        );
    }

    #[test]
    fn statistics_use_interpolated_order_values() {
        assert_eq!(percentile(&[], 0.5), None);
        assert_eq!(percentile(&[4.0, 1.0, 3.0, 2.0], 0.5), Some(2.5));
        assert!((percentile(&[1.0, 2.0, 3.0, 4.0], 0.95).unwrap() - 3.85).abs() < 1e-12);
    }

    #[test]
    fn an_incomplete_requested_deciding_suite_is_not_a_pass() {
        let report = summarize(&[], true, 10);
        assert_eq!(report["deciding"], false);
        assert_eq!(report["complete"], false);
        assert_eq!(report["verdict"], "incomplete suite");
    }

    #[test]
    fn fixture_credentials_need_private_permissions_and_exact_identity() {
        let fixture = OwnedDirectory::new().unwrap();
        let file = fixture.0.join("fixture.json");
        fs::write(
            &file,
            br#"{"account":"Fixture","password":"discarded-test-data"}"#,
        )
        .unwrap();
        let expected = "Fixture".to_owned();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(fixture_credentials(&file, Some(&expected)).is_err());
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(fixture_credentials(&file, Some(&"Other".to_owned())).is_err());
        assert_eq!(
            fixture_credentials(&file, Some(&expected)).unwrap().account,
            expected
        );
    }

    #[test]
    fn immutable_copy_is_digest_bound_and_preserves_existing_files() {
        let fixture = OwnedDirectory::new().unwrap();
        let source = fixture.0.join("source.so");
        fs::write(&source, b"abc").unwrap();
        let (target, digest) = immutable(&source, &fixture.0.join("artifacts"), "test").unwrap();
        assert_eq!(
            digest,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(&target, b"changed").unwrap();
        assert!(immutable(&source, &fixture.0.join("artifacts"), "test").is_err());
        assert_eq!(fs::read(&target).unwrap(), b"changed");
    }

    #[test]
    fn cache_copy_refuses_symlinks_without_following_them() {
        let fixture = OwnedDirectory::new().unwrap();
        let template = fixture.0.join("template");
        fs::create_dir(&template).unwrap();
        let sentinel = fixture.0.join("sentinel");
        fs::write(&sentinel, "retained").unwrap();
        std::os::unix::fs::symlink(&sentinel, template.join("link")).unwrap();
        assert!(copy_tree(&template, &fixture.0.join("copy")).is_err());
        assert_eq!(fs::read_to_string(sentinel).unwrap(), "retained");
    }

    #[test]
    fn owned_process_drop_stops_and_reaps_only_its_child() {
        let child = Process(Command::new("sleep").arg("30").spawn().unwrap());
        let pid = child.0.id();
        assert!(PathBuf::from(format!("/proc/{pid}")).exists());
        drop(child);
        assert!(!PathBuf::from(format!("/proc/{pid}")).exists());
    }

    #[test]
    fn owned_directory_drop_removes_its_fixture() {
        let fixture = OwnedDirectory::new().unwrap();
        let path = fixture.0.clone();
        fs::write(path.join("owned"), "fixture").unwrap();
        drop(fixture);
        assert!(!path.exists());
    }
}
