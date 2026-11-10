use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    env,
    ffi::OsString,
    fs::{self, File},
    io::Read,
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

const HELP: &str = "luabuddy-performance --game-dir DIR --runtime FILE --observer FILE --out DIR [--template-cache DIR] [--mode menu|world] [--rounds 5] [--warmup 10] [--seconds 30] [--deadline 300] [--single baseline|enabled] [--jvm-args JSONFILE] [--game-args JSONFILE] [--auto-ui] [--account NAME] [--prime-cache-out DIR] [--deciding]\nA deciding suite uses five alternating runs per arm, at least30 seconds measured per run, and an immutable release runtime. Native automation clicks only observed controls in its own client.\n";

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
}

impl Options {
    fn parse() -> Result<Self> {
        let mut args = env::args().skip(1);
        let mut values = BTreeMap::new();
        let mut deciding = false;
        let mut auto_ui = false;
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
        };
        if !matches!(result.mode.as_str(), "menu" | "world")
            || !(1..=20).contains(&result.rounds)
            || !(1..=120).contains(&result.warmup)
            || !(1..=300).contains(&result.seconds)
            || !(30..=900).contains(&result.deadline)
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
        if result.deciding
            && (result.single.is_some()
                || result.rounds < 5
                || result.seconds < 30
                || !result.runtime.to_string_lossy().contains("release"))
        {
            return Err("Deciding evidence needs five paired runs, at least30 measured seconds, and a release runtime path".into());
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
    let work = OwnedDirectory::new()?;
    let cache = work.0.join("cache");
    seed_cache(options, &cache)?;
    fs::create_dir_all(work.0.join("profiles"))?;
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
        &json!({"schema":1,"output":report,"enabled":enabled,"mode":options.mode,"warmup_seconds":options.warmup,"measure_seconds":options.seconds,"progress":out.join("controls.json"),"account":options.account,"hud":out.join("hud/frametime_(ms)")}),
    )?;
    let (_display_process, display, authority) = display(&work.0, &out)?;
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
        "-Dzomboid.steam=0".into(),
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
        "-nosteam".into(),
        "-nosound".into(),
        "-novoip".into(),
        format!("-cachedir={}", cache.display()),
    ]);
    arguments.extend(options.game_args.clone());
    let log = File::create(out.join("client.log"))?;
    let mut path = OsString::from(options.game.join("jre64/bin"));
    path.push(":");
    path.push(env::var_os("PATH").unwrap_or_default());
    let mut libraries = OsString::from(options.game.join("natives"));
    libraries.push(":");
    libraries.push(env::var_os("LD_LIBRARY_PATH").unwrap_or_default());
    let launched_wall = wall_ms()?;
    let mut child = Process(
        Command::new(options.game.join("jre64/bin/java"))
            .args(&arguments)
            .current_dir(&options.game)
            .env("DISPLAY", &display)
            .env("XAUTHORITY", &authority)
            .env("LIBGL_ALWAYS_SOFTWARE", "1")
            .env("GALLIUM_HUD", "frametime")
            .env("GALLIUM_HUD_PERIOD", "0")
            .env("GALLIUM_HUD_VISIBLE", "false")
            .env("GALLIUM_HUD_DUMP_DIR", out.join("hud"))
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
        if options.auto_ui {
            perform_control(
                &out.join("controls.json"),
                child.0.id(),
                &display,
                &authority,
                &mut actions,
                options.credentials.as_ref(),
            )?;
        }
        if !focused
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
        thread::sleep(Duration::from_millis(250));
    }
    samples.push(process_sample(child.0.id(), page, ticks)?);
    let frames: Value = serde_json::from_slice(&fs::read(&report)?)?;
    if !frames["error"].is_null() || frames["frame_count"].as_u64().unwrap_or(0) < 2 {
        return Err(format!("Performance observer did not complete valid work: {frames}").into());
    }
    let image = out.join("usable-workload.png");
    let _ = Command::new("import")
        .args(["-window", "root"])
        .arg(&image)
        .env("DISPLAY", &display)
        .env("XAUTHORITY", &authority)
        .status();
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
    let content = fs::read_to_string(out.join("client.log"))?;
    let errors = content
        .lines()
        .filter(|line| {
            line.contains("[LuaBuddy performance] FAIL")
                || line.contains("WARNING in native method")
                || line.contains("FATAL ERROR")
                || line.contains("[LuaBuddy] startup inactive")
        })
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let record = json!({"schema":1,"arm":arm,"index":index,"ok":errors.is_empty(),"errors":errors,"startupMs":frames["ready_unix_ms"].as_f64().ok_or("Missing readiness clock")?-launched_wall,"observerStartupMs":frames["startup_ms"],"frameCount":timings.len(),"p50Ms":percentile(&timings,0.5),"p95Ms":percentile(&timings,0.95),"p99Ms":percentile(&timings,0.99),"cpuSeconds":cpu,"cpuWindowSeconds":(last["unixMs"].as_f64().ok_or("Missing process clock")?-first["unixMs"].as_f64().ok_or("Missing process clock")?)/1000.0,"rssMedianBytes":percentile(&rss,0.5),"rssMaxBytes":rss.iter().copied().max_by(f64::total_cmp),"measurement":frames,"machineLoad":fs::read_to_string("/proc/loadavg")?,"focused":focused});
    write(&out.join("process-samples.json"), &json!(samples))?;
    write(&out.join("result.json"), &record)?;
    println!(
        "COMPLETE {index} {arm} frames={} p95Ms={} startupMs={}",
        record["frameCount"], record["p95Ms"], record["startupMs"]
    );
    if !errors.is_empty() {
        return Err(format!("Owned {arm} client logged observer/JNI failures").into());
    }
    if options.auto_ui {
        let cleanup_deadline = Instant::now() + Duration::from_secs(30);
        while child.0.try_wait()?.is_none() && Instant::now() < cleanup_deadline {
            perform_control(
                &out.join("controls.json"),
                child.0.id(),
                &display,
                &authority,
                &mut actions,
                options.credentials.as_ref(),
            )?;
            thread::sleep(Duration::from_millis(200));
        }
    }
    let graceful = child.0.try_wait()?.is_some_and(|status| status.success());
    if let Some(destination) = &options.prime {
        if !graceful {
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
        fs::create_dir(destination)?;
        fs::set_permissions(destination, fs::Permissions::from_mode(0o700))?;
        copy_tree(&cache, destination)?;
    }
    write(
        &out.join("cleanup.json"),
        &json!({"nativeGracefulExit":graceful,"primedCache":options.prime,"performedNativeControls":actions}),
    )?;
    Ok(record)
}

fn key(display: &str, authority: &Path, value: &str) -> Result<()> {
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
    Ok(())
}

fn perform_control(
    path: &Path,
    pid: u32,
    display: &str,
    authority: &Path,
    done: &mut std::collections::BTreeSet<String>,
    credentials: Option<&FixtureCredentials>,
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
    if done.contains(id) && id != "MainScreen.open-pause-menu" {
        return Ok(());
    }
    match (value["action"].as_str(), id) {
        (Some("fixture-login"), "ServerConnectPopup.fixture-credentials") => {
            let credentials = credentials.ok_or(
                "The native popup needs the explicitly supplied disposable fixture credentials",
            )?;
            input_fixture(
                display,
                authority,
                &value["password"],
                &credentials.password,
            )?;
            input_fixture(display, authority, &value["username"], &credentials.account)?;
        }
        (
            Some("click"),
            "ServerConnectPopup.instance.connectBtn"
            | "GameLoadingState.click-to-start"
            | "MainScreen.nativeQuitLabel"
            | "MainScreen.quitToDesktopDialog.yes",
        ) => {
            let x = value["x"]
                .as_f64()
                .filter(|v| v.is_finite() && (0.0..1280.0).contains(v))
                .ok_or("Invalid native control X")?;
            let y = value["y"]
                .as_f64()
                .filter(|v| v.is_finite() && (0.0..800.0).contains(v))
                .ok_or("Invalid native control Y")?;
            if !Command::new("xdotool")
                .args(["mousemove", &format!("{x:.0}"), &format!("{y:.0}")])
                .env("DISPLAY", display)
                .env("XAUTHORITY", authority)
                .status()?
                .success()
            {
                return Err("Owned native control positioning failed".into());
            }
            thread::sleep(Duration::from_millis(150));
            if !Command::new("xdotool")
                .args(["mousedown", "1"])
                .env("DISPLAY", display)
                .env("XAUTHORITY", authority)
                .status()?
                .success()
            {
                return Err("Owned native click-down failed".into());
            }
            thread::sleep(Duration::from_millis(250));
            if !Command::new("xdotool")
                .args(["mouseup", "1"])
                .env("DISPLAY", display)
                .env("XAUTHORITY", authority)
                .status()?
                .success()
            {
                return Err("Owned native click-up failed".into());
            }
        }
        (Some("key"), "MainScreen.open-pause-menu") => {
            key(display, authority, "Escape")?;
            thread::sleep(Duration::from_secs(1));
        }
        (Some("key"), "SurvivalGuide.native-dismiss") => key(display, authority, "F1")?,
        _ => return Err("Unrecognized native verification control".into()),
    }
    done.insert(id.to_owned());
    println!("NATIVE CONTROL {id}");
    Ok(())
}

fn input_fixture(display: &str, authority: &Path, field: &Value, text: &str) -> Result<()> {
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
    key(display, authority, "ctrl+a")?;
    if !Command::new("xdotool")
        .args(["type", "--clearmodifiers", "--delay", "80", "--", text])
        .env("DISPLAY", display)
        .env("XAUTHORITY", authority)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?
        .success()
    {
        return Err(
            "Typing explicitly supplied fixture data into the owned native field failed".into(),
        );
    }
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
    let settings = json!({"schema":1,"gameJarSha256":hash(&options.game.join("projectzomboid.jar"))?,"runtimeSha256":runtime_hash,"observerSha256":observer_hash,"mode":options.mode,"roundsPerArm":options.rounds,"warmupSeconds":options.warmup,"measurementSeconds":options.seconds,"deciding":options.deciding,"cpuCount":std::thread::available_parallelism()?.get(),"initialLoad":fs::read_to_string("/proc/loadavg")?,"jvmRelease":fs::read_to_string(options.game.join("jre64/release"))?,"observer":"identical hidden Mesa Gallium per-frame dump plus read-only JVMTI agent thread; no Java code edits, breakpoints, or global tracing"});
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
                        &summarize(&records, options.deciding, options.rounds * arms.len()),
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
    let summary = summarize(&records, options.deciding, options.rounds * arms.len());
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
