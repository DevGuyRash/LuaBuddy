use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const OUTPUT_LIMIT: usize = 16 * 1024 * 1024;
const ARTIFACT_LIMIT: u64 = 256 * 1024 * 1024;
const HELP: &str = "LuaBuddy verification\n\n  cargo xtask check [--offline] [--portable] [--game-dir DIR] [--package DIR] [--runtime FILE] [--cli FILE] [--audit] [--log-dir DIR]\n\nBuilds the application before tests, checks formatting and Clippy, runs contract tests, verifies compiled extensions and a native package. Native checks use the selected game's bundled JVM and disposable fixtures. --runtime and --cli select immutable snapshots of deciding artifacts, such as release binaries; omitted values use the completed debug build. An existing --package must contain those exact runtime and CLI bytes. --portable explicitly omits proprietary-game checks. --audit requires cargo-audit and a usable advisory database. --offline prevents dependency and advisory database fetches. No check subscribes to or downloads Workshop items, controls a running game, or launches a graphical client.\n";

#[derive(Default)]
struct Options {
    offline: bool,
    portable: bool,
    audit: bool,
    game: Option<PathBuf>,
    package: Option<PathBuf>,
    runtime: Option<PathBuf>,
    cli: Option<PathBuf>,
    logs: Option<PathBuf>,
}

impl Options {
    fn parse() -> Result<Option<Self>> {
        let mut args = env::args_os().skip(1);
        match args.next().as_deref() {
            None => return Ok(None),
            Some(s) if s == "help" || s == "--help" || s == "-h" => return Ok(None),
            Some(s) if s == "check" => (),
            _ => return Err("Expected check or --help".into()),
        }
        let mut options = Self::default();
        while let Some(arg) = args.next() {
            match arg.to_str() {
                Some("--offline") => options.offline = true,
                Some("--portable") => options.portable = true,
                Some("--audit") => options.audit = true,
                Some("--game-dir") => {
                    options.game =
                        Some(args.next().ok_or("--game-dir requires a directory")?.into())
                }
                Some("--package") => {
                    options.package =
                        Some(args.next().ok_or("--package requires a directory")?.into())
                }
                Some("--runtime") => {
                    options.runtime = Some(args.next().ok_or("--runtime requires a file")?.into())
                }
                Some("--cli") => {
                    options.cli = Some(args.next().ok_or("--cli requires a file")?.into())
                }
                Some("--log-dir") => {
                    options.logs = Some(args.next().ok_or("--log-dir requires a directory")?.into())
                }
                Some("--help" | "-h") => return Ok(None),
                _ => return Err(format!("Unknown argument {arg:?}").into()),
            }
        }
        if options.portable && options.game.is_some() {
            return Err("--portable and --game-dir select different verification scopes".into());
        }
        Ok(Some(options))
    }
}

#[derive(Debug)]
struct Capture {
    success: bool,
    stdout: String,
    stderr: String,
}

impl Capture {
    fn transcript(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }
}

fn proof_clean(output: &Capture, markers: &[&str]) -> bool {
    let text = output.transcript();
    output.success
        && markers.iter().all(|marker| text.contains(marker))
        && (markers.is_empty()
            || ![
                "FAIL",
                "WARNING in native method",
                "WARNING: JNI",
                "FATAL ERROR",
                "A fatal error has been detected",
            ]
            .iter()
            .any(|marker| text.contains(marker)))
}

struct Checks {
    root: PathBuf,
    logs: PathBuf,
    records: Vec<Value>,
    failed: bool,
    provenance: Value,
}

struct Artifact {
    source: PathBuf,
    snapshot: PathBuf,
    digest: String,
    size: usize,
}

impl Artifact {
    fn snapshot(source: PathBuf, destination: PathBuf) -> Result<Self> {
        let source = source.canonicalize()?;
        let metadata = fs::metadata(&source)?;
        if !metadata.is_file() || metadata.len() > ARTIFACT_LIMIT {
            return Err(format!(
                "Verification artifact must be a file of at most 256 MiB: {}",
                source.display()
            )
            .into());
        }
        let mut content = Vec::new();
        fs::File::open(&source)?
            .take(ARTIFACT_LIMIT + 1)
            .read_to_end(&mut content)?;
        if content.len() as u64 > ARTIFACT_LIMIT {
            return Err("Verification artifact grew beyond 256 MiB while being read".into());
        }
        let digest = format!("{:x}", Sha256::digest(&content));
        fs::write(&destination, &content)?;
        fs::set_permissions(&destination, fs::metadata(&source)?.permissions())?;
        Ok(Self {
            source,
            snapshot: destination,
            digest,
            size: content.len(),
        })
    }

    fn provenance(&self) -> Value {
        json!({"source":self.source,"sha256":self.digest,"size":self.size,"testedFrom":"immutable owned snapshot"})
    }
}

struct Artifacts {
    runtime: Artifact,
    cli: Artifact,
}

impl Artifacts {
    fn snapshot(root: &Path, directory: &Path, options: &Options) -> Result<Self> {
        Ok(Self {
            runtime: Artifact::snapshot(
                options
                    .runtime
                    .clone()
                    .unwrap_or_else(|| root.join("target/debug").join(library("luabuddy_host"))),
                directory.join(library("luabuddy_host")),
            )?,
            cli: Artifact::snapshot(
                options
                    .cli
                    .clone()
                    .unwrap_or_else(|| executable(root, "luabuddy")),
                directory.join(format!("luabuddy{}", env::consts::EXE_SUFFIX)),
            )?,
        })
    }

    fn provenance(&self) -> Value {
        json!({"runtime":self.runtime.provenance(),"cli":self.cli.provenance()})
    }
}

struct Disposable(PathBuf);

impl Disposable {
    fn new(path: PathBuf) -> Result<Self> {
        fs::create_dir(&path)?;
        Ok(Self(path))
    }
    fn remove(self) -> Result<()> {
        fs::remove_dir_all(&self.0)?;
        Ok(())
    }
}

impl Drop for Disposable {
    fn drop(&mut self) {
        if self.0.exists()
            && let Err(error) = fs::remove_dir_all(&self.0)
        {
            eprintln!(
                "Cannot remove owned verification fixture {}: {error}",
                self.0.display()
            );
        }
    }
}

impl Checks {
    fn note(&mut self, label: &str, state: &str, detail: &str) {
        println!(
            "{state}: {label}{separator}{detail}",
            separator = if detail.is_empty() { "" } else { " | " }
        );
        self.failed |= state == "FAILED";
        self.records
            .push(json!({"check":label,"status":state,"detail":detail}));
    }

    fn cargo(&self, args: &[&str], offline: bool) -> Command {
        let mut command = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
        command
            .current_dir(&self.root)
            .args(args)
            .env("CARGO_TARGET_DIR", self.root.join("target"));
        if offline {
            command.arg("--offline");
        }
        command
    }

    fn command(
        &mut self,
        label: &str,
        mut command: Command,
        timeout: Duration,
        markers: &[&str],
    ) -> Option<Capture> {
        let number = self.records.len() + 1;
        let log = self.logs.join(format!("{number:02}.log"));
        let description = format!("{command:?}");
        let result = capture(&mut command, timeout);
        match result {
            Ok(output) => {
                let text = output.transcript();
                let clean = proof_clean(&output, markers);
                if let Err(error) = fs::write(&log, format!("{description}\n\n{text}")) {
                    self.note(label, "FAILED", &format!("Cannot retain evidence: {error}"));
                    return None;
                }
                if clean {
                    self.note(label, "PASSED", &log.display().to_string());
                    Some(output)
                } else {
                    self.note(label, "FAILED", &log.display().to_string());
                    eprintln!("{text}");
                    None
                }
            }
            Err(error) => {
                let detail = error.to_string();
                let _ = fs::write(&log, format!("{description}\n\n{detail}\n"));
                self.note(label, "FAILED", &format!("{detail}; {}", log.display()));
                None
            }
        }
    }

    fn finish(self) -> Result<bool> {
        let report = self.logs.join("report.json");
        fs::write(
            &report,
            serde_json::to_vec_pretty(
                &json!({"schemaVersion":1,"passed":!self.failed,"provenance":self.provenance,"checks":self.records}),
            )?,
        )?;
        println!("Evidence: {}", report.display());
        Ok(!self.failed)
    }
}

fn inputs(root: &Path, logs: &Path) -> Result<Vec<(String, Option<Vec<u8>>)>> {
    let mut command = Command::new("git");
    command.current_dir(root).args([
        "ls-files",
        "--cached",
        "--others",
        "--exclude-standard",
        "-z",
    ]);
    let output = capture(&mut command, Duration::from_secs(15))?;
    if !output.success {
        return Err(format!("Cannot record verification inputs: {}", output.stderr).into());
    }
    let mut names = output
        .stdout
        .split('\0')
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    let logs = logs.canonicalize()?;
    names
        .into_iter()
        .filter(|name| !root.join(name).starts_with(&logs))
        .map(|name| {
            let path = root.join(&name);
            let content = match fs::read(&path) {
                Ok(content) => Some(content),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => {
                    return Err(
                        format!("Read verification input {}: {error}", path.display()).into(),
                    );
                }
            };
            Ok((name, content))
        })
        .collect()
}

fn reader(mut pipe: impl Read) -> std::io::Result<(Vec<u8>, bool)> {
    let mut captured = Vec::new();
    let mut exceeded = false;
    let mut buffer = [0; 8192];
    loop {
        let count = pipe.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let keep = count.min(OUTPUT_LIMIT.saturating_sub(captured.len()));
        captured.extend_from_slice(&buffer[..keep]);
        exceeded |= keep != count;
    }
    Ok((captured, exceeded))
}

fn capture(command: &mut Command, timeout: Duration) -> Result<Capture> {
    command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn()?;
    let stdout = child.stdout.take().ok_or("Child stdout was not captured")?;
    let stderr = child.stderr.take().ok_or("Child stderr was not captured")?;
    let stdout = thread::spawn(move || reader(stdout));
    let stderr = thread::spawn(move || reader(stderr));
    let started = Instant::now();
    let mut expired = false;
    let status = loop {
        if stdout.is_finished()
            && stderr.is_finished()
            && let Some(status) = child.try_wait()?
        {
            break status;
        }
        if started.elapsed() >= timeout {
            expired = true;
            stop_tree(child.id());
            let _ = child.kill();
            let status = child.wait()?;
            let draining = Instant::now();
            while !stdout.is_finished() || !stderr.is_finished() {
                if draining.elapsed() > Duration::from_secs(5) {
                    return Err("Command exceeded its deadline and descendant output remained open after cancellation".into());
                }
                thread::sleep(Duration::from_millis(10));
            }
            break status;
        }
        thread::sleep(Duration::from_millis(25));
    };
    let (stdout, stdout_exceeded) = stdout.join().map_err(|_| "stdout reader panicked")??;
    let (stderr, stderr_exceeded) = stderr.join().map_err(|_| "stderr reader panicked")??;
    if expired {
        return Err(format!(
            "Command exceeded its {}-second deadline; its owned process tree was stopped",
            timeout.as_secs()
        )
        .into());
    }
    if stdout_exceeded || stderr_exceeded {
        return Err("Command exceeded the 16 MiB output limit".into());
    }
    Ok(Capture {
        success: status.success(),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

fn stop_tree(pid: u32) {
    #[cfg(unix)]
    {
        let group = format!("-{pid}");
        let _ = Command::new("kill")
            .args(["-KILL", "--", &group])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

fn library(name: &str) -> String {
    format!(
        "{}{}{}",
        env::consts::DLL_PREFIX,
        name,
        env::consts::DLL_SUFFIX
    )
}

fn executable(root: &Path, name: &str) -> PathBuf {
    root.join("target/debug")
        .join(format!("{name}{}", env::consts::EXE_SUFFIX))
}

fn file_digest(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn package_matches(package: &Path, artifacts: &Artifacts) -> Result<()> {
    let manifest: Value = serde_json::from_slice(&fs::read(package.join("manifest.json"))?)?;
    let runtime = manifest["runtime"]
        .as_str()
        .ok_or("Package has no runtime identity")?;
    let runtime_path = Path::new(runtime);
    if runtime_path.components().count() != 1
        || !matches!(
            runtime_path.components().next(),
            Some(std::path::Component::Normal(_))
        )
    {
        return Err("Package runtime identity is not a root filename".into());
    }
    let cli = if manifest["os"] == "windows" {
        "luabuddy.exe"
    } else {
        "luabuddy"
    };
    for (name, expected) in [
        (runtime, &artifacts.runtime.digest),
        (cli, &artifacts.cli.digest),
    ] {
        if manifest["files"][name]["sha256"].as_str() != Some(expected)
            || file_digest(&package.join(name))? != *expected
        {
            return Err(format!(
                "Package {name} does not match the selected deciding artifact SHA256 {expected}"
            )
            .into());
        }
    }
    Ok(())
}

fn jvm_property(text: &str, name: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let (key, value) = line.trim().split_once('=')?;
        (key.trim() == name).then(|| value.trim().to_owned())
    })
}

fn native_environment(command: &mut Command, game: &Path, errors: &Path) -> Result<()> {
    let java_home = game.join("jre64");
    let java = java_home
        .join("bin")
        .join(format!("java{}", env::consts::EXE_SUFFIX));
    if !java.is_file() {
        return Err(format!(
            "Selected game has no bundled JVM executable at {}",
            java.display()
        )
        .into());
    }
    let mut path = vec![java_home.join("bin")];
    path.extend(env::split_paths(&env::var_os("PATH").unwrap_or_default()));
    let errors = errors
        .to_str()
        .ok_or("JVM error evidence path must be UTF-8")?;
    if errors.contains(['"', '\\', '\n', '\r']) {
        return Err(
            "JVM error evidence path contains an unsupported option quote or escape".into(),
        );
    }
    command
        .env("JAVA_HOME", &java_home)
        .env("PATH", env::join_paths(path)?)
        .env("JAVA_TOOL_OPTIONS", format!("--enable-native-access=ALL-UNNAMED -XX:ErrorFile=\"{errors}/hs_err_pid%p.log\" -XX:-CreateCoredumpOnCrash"));
    #[cfg(target_os = "linux")]
    {
        let mut libraries = vec![game.join("natives")];
        libraries.extend(env::split_paths(
            &env::var_os("LD_LIBRARY_PATH").unwrap_or_default(),
        ));
        command.env("LD_LIBRARY_PATH", env::join_paths(libraries)?);
    }
    Ok(())
}

fn package_scan(path: &Path) -> Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.file_type()?;
        if metadata.is_symlink() {
            return Err(format!("Package contains a symlink: {}", entry.path().display()).into());
        }
        if metadata.is_dir() {
            package_scan(&entry.path())?;
        } else if ["lua", "py", "java", "jar", "class", "js", "wasm"]
            .iter()
            .any(|extension| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|actual| actual.eq_ignore_ascii_case(extension))
            })
        {
            return Err(format!(
                "Package contains a prohibited implementation payload: {}",
                entry.path().display()
            )
            .into());
        }
    }
    Ok(())
}

fn standalone_build(checks: &mut Checks, manifest: &str, target: &str, offline: bool) -> bool {
    let mut command = checks.cargo(
        &[
            "build",
            "--manifest-path",
            manifest,
            "--locked",
            "--target-dir",
            target,
        ],
        offline,
    );
    command.env("CARGO_TARGET_DIR", checks.root.join(target));
    checks
        .command(
            &format!("Build {manifest}"),
            command,
            Duration::from_secs(600),
            &[],
        )
        .is_some()
}

fn native_checks(checks: &mut Checks, options: &Options, artifacts: &Artifacts) -> Result<()> {
    if options.portable {
        checks.note(
            "Native game/JVM proofs",
            "NOT RUN",
            "Explicit portable scope",
        );
        return Ok(());
    }
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        checks.note(
            "Native game/JVM proofs",
            "NOT RUN",
            "Only Linux x86_64 has an implemented and observed native adapter",
        );
        checks.failed = true;
        return Ok(());
    }
    let game = if let Some(game) = &options.game {
        game.canonicalize()?
    } else {
        let mut command = Command::new(&artifacts.cli.snapshot);
        command.current_dir(&checks.root).arg("discover");
        let Some(output) = checks.command(
            "Discover native host",
            command,
            Duration::from_secs(30),
            &[],
        ) else {
            return Ok(());
        };
        let result: Value = serde_json::from_str(&output.stdout)?;
        PathBuf::from(
            result["game"]
                .as_str()
                .ok_or("Discovery did not return one game path")?,
        )
        .canonicalize()?
    };
    let game = if game.join("projectzomboid.jar").is_file() {
        game
    } else {
        game.join("projectzomboid").canonicalize()?
    };
    let java = game.join("jre64/bin/java");
    let jar = game.join("projectzomboid.jar");
    if !jar.is_file() {
        return Err("Native proofs currently require the observed single-JAR game layout".into());
    }
    let errors = checks.logs.join("jvm-errors");
    fs::create_dir_all(&errors)?;
    let mut version = Command::new(&java);
    version.args(["-XshowSettings:properties", "-version"]);
    native_environment(&mut version, &game, &errors)?;
    if let Some(output) = checks.command(
        "Actual bundled JVM version and process architecture",
        version,
        Duration::from_secs(30),
        &["java.version", "os.arch"],
    ) {
        let text = output.transcript();
        checks.provenance["host"] = json!({
            "gameDirectory":game,"gameJarSha256":file_digest(&jar)?,"javaExecutable":java,
            "javaVersion":jvm_property(&text,"java.version"),"jvmArchitecture":jvm_property(&text,"os.arch"),
            "jvmDataModel":jvm_property(&text,"sun.arch.data.model")
        });
    }
    let home = checks.logs.join("native-home");
    let fixture = Disposable::new(home.clone())?;
    let mut proof = Command::new(&java);
    proof
        .current_dir(&game)
        .args([
            "-Xcheck:jni",
            "-Xverify:all",
            "--enable-native-access=ALL-UNNAMED",
            "-Dzomboid.steam=0",
        ])
        .arg(format!("-Duser.home={}", home.display()))
        .arg(format!(
            "-Djava.library.path={}",
            game.join("natives").display()
        ))
        .arg(format!(
            "-agentpath:{}=proof-native",
            artifacts.runtime.snapshot.display()
        ))
        .arg("-cp")
        .arg(&jar)
        .arg("-version");
    native_environment(&mut proof, &game, &errors)?;
    if let Some(output) = checks.command(
        "Native callable, dispatcher, UI and download notification proof",
        proof,
        Duration::from_secs(60),
        &[
            "PASS Rust callable",
            "PASS reentrant",
            "PASS native callback exception",
            "PASS actual dispatch",
            "PASS actual UIElement",
            "PASS actual Workshop completion callbacks",
        ],
    ) {
        checks.provenance["host"]["gameVersion"] = output
            .transcript()
            .lines()
            .find_map(|line| line.strip_prefix("[LuaBuddy native proof] tested game "))
            .map(str::to_owned)
            .into();
    }
    for (name, marker) in [
        ("metadata", "PASS: real cache report"),
        ("runtime", "PASS production runtime parity proof"),
        (
            "activation",
            "PASS native startup selection and centralized live eligibility",
        ),
        (
            "steam",
            "PASS: actual host SteamUtils rejects Workshop access",
        ),
    ] {
        let manifest = format!("proofs/{name}/Cargo.toml");
        let target = format!("target/verify-{name}");
        if standalone_build(checks, &manifest, &target, options.offline) {
            let mut command = Command::new(
                checks
                    .root
                    .join(&target)
                    .join("debug")
                    .join(format!("luabuddy-{name}-proof")),
            );
            command.current_dir(&checks.root).arg(&game);
            if name == "runtime" {
                command.arg(&artifacts.runtime.snapshot);
            }
            native_environment(&mut command, &game, &errors)?;
            checks.command(
                &format!("Actual {name} proof"),
                command,
                Duration::from_secs(120),
                &[marker],
            );
        } else {
            checks.note(
                &format!("Actual {name} proof"),
                "NOT RUN",
                "Its proof did not build",
            );
        }
    }
    if standalone_build(
        checks,
        "proofs/patcher/Cargo.toml",
        "target/patcher-proof",
        options.offline,
    ) {
        let mut command = Command::new(&java);
        command
            .current_dir(&game)
            .args([
                "-Xcheck:jni",
                "-Xverify:all",
                "--enable-native-access=ALL-UNNAMED",
            ])
            .arg(format!(
                "-agentpath:{}=proof-patcher",
                artifacts.runtime.snapshot.display()
            ))
            .arg(format!("-Duser.home={}", home.display()))
            .arg(format!(
                "-Dluabuddy.proof.math={}",
                checks
                    .root
                    .join("target/extensions/debug")
                    .join(library("luabuddy_math_guard"))
                    .display()
            ))
            .arg(format!(
                "-Dluabuddy.proof.observer={}",
                checks
                    .root
                    .join("target/extensions-observer/debug")
                    .join(library("luabuddy_string_observer"))
                    .display()
            ))
            .arg("-cp")
            .arg(&jar)
            .arg("-version");
        native_environment(&mut command, &game, &errors)?;
        checks.command(
            "Actual JVM patcher and independent compiled extensions",
            command,
            Duration::from_secs(60),
            &["[LuaBuddy patcher proof] PASS live JVM patches"],
        );
    }
    let marker_built = standalone_build(
        checks,
        "proofs/extensions/marker/Cargo.toml",
        "target/extensions-marker",
        options.offline,
    );
    let extensions_built = standalone_build(
        checks,
        "proofs/extensions/Cargo.toml",
        "target/extensions-product-proof",
        options.offline,
    );
    if marker_built && extensions_built {
        let mut command = Command::new(
            checks
                .root
                .join("target/extensions-product-proof/debug/luabuddy-extension-product-proof"),
        );
        command
            .current_dir(&checks.root)
            .arg(&game)
            .arg(&checks.root)
            .env("LUABUDDY_PROOF_RUNTIME", &artifacts.runtime.snapshot)
            .env("LUABUDDY_PROOF_CLI", &artifacts.cli.snapshot);
        native_environment(&mut command, &game, &errors)?;
        checks.command("Normal product extension approval, activation and explicit retry", command, Duration::from_secs(360), &[
            "PASS normal product extension disabled:", "PASS normal product extension unapproved:",
            "PASS normal product extension mismatch:", "PASS normal product extension manifest-tamper:",
            "PASS normal product extension metadata-update:", "PASS normal product extension active:",
            "PASS normal product extension deferred:",
            "PASS normal production-agent extension delivery matrix; no account or user-game changes",
        ]);
    } else {
        checks.note(
            "Normal product extension approval, activation and explicit retry",
            "NOT RUN",
            "Its consumer or constructor observation fixture did not build",
        );
    }
    if standalone_build(
        checks,
        "proofs/performance/Cargo.toml",
        "target/verify-performance",
        options.offline,
    ) {
        let mut command = checks.cargo(
            &[
                "test",
                "--manifest-path",
                "proofs/performance/Cargo.toml",
                "--locked",
                "--target-dir",
                "target/verify-performance",
            ],
            options.offline,
        );
        command.env(
            "CARGO_TARGET_DIR",
            checks.root.join("target/verify-performance"),
        );
        checks.command(
            "Performance harness ownership and provenance tests",
            command,
            Duration::from_secs(120),
            &[],
        );
    }
    if standalone_build(
        checks,
        "proofs/bridge-loader/Cargo.toml",
        "target/bridge-loader-proof",
        options.offline,
    ) {
        let mut command = Command::new(&java);
        command
            .current_dir(&game)
            .args([
                "-Xcheck:jni",
                "-Xverify:all",
                "--enable-native-access=ALL-UNNAMED",
            ])
            .arg(format!("-Duser.home={}", home.display()))
            .arg(format!("-Dluabuddy.proof.jar={}", jar.display()))
            .arg(format!(
                "-agentpath:{}",
                checks
                    .root
                    .join("target/bridge-loader-proof/debug")
                    .join(library("luabuddy_bridge_loader_proof"))
                    .display()
            ))
            .arg("-cp")
            .arg(&jar)
            .arg("-version");
        native_environment(&mut command, &game, &errors)?;
        checks.command(
            "Defining-classloader isolation regression",
            command,
            Duration::from_secs(60),
            &["[LuaBuddy bridge loader proof] PASS"],
        );
    }
    fixture.remove()?;
    checks.note(
        "Graphical gameplay and performance",
        "NOT RUN",
        "Require the separate disposable-client workflow and matched performance protocol",
    );
    checks.note(
        "Workshop account mutations",
        "NOT RUN",
        "The user authorized read-only subscribed-item checks only",
    );
    checks.note(
        "Subscribe-only native activation",
        "NOT RUN",
        "Workshop delivery does not establish native activation",
    );
    Ok(())
}

fn run(options: Options) -> Result<bool> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("Verifier has no workspace parent")?
        .to_path_buf();
    let logs = match &options.logs {
        Some(logs) => logs.clone(),
        None => {
            let state = env::var_os("XDG_STATE_HOME")
                .map(PathBuf::from)
                .or_else(|| {
                    env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
                })
                .unwrap_or_else(env::temp_dir);
            state.join("app-verification/LuaBuddy").join(format!(
                "aggregate-{}-{}",
                SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
                std::process::id()
            ))
        }
    };
    fs::create_dir_all(&logs)?;
    let logs = logs.canonicalize()?;
    let mut checks = Checks {
        root,
        logs,
        records: Vec::new(),
        failed: false,
        provenance: json!({"platform":{"os":env::consts::OS,"architecture":env::consts::ARCH},"scope":{"portable":options.portable,"graphical":false,"accountMutations":false,"matchedPerformance":false}}),
    };
    let before = inputs(&checks.root, &checks.logs)?;
    let build = checks.cargo(
        &[
            "build",
            "--workspace",
            "--all-targets",
            "--exclude",
            "luabuddy-verify",
            "--locked",
        ],
        options.offline,
    );
    let built = checks
        .command(
            "Build complete application",
            build,
            Duration::from_secs(600),
            &[],
        )
        .is_some();
    let format = checks.cargo(&["fmt", "--all", "--", "--check"], false);
    checks.command("Workspace formatting", format, Duration::from_secs(60), &[]);
    let mut lint = checks.cargo(
        &["clippy", "--workspace", "--all-targets", "--locked"],
        options.offline,
    );
    lint.args(["--", "-D", "warnings"]);
    checks.command(
        "Workspace strict Clippy",
        lint,
        Duration::from_secs(600),
        &[],
    );
    if built {
        let artifact_fixture = Disposable::new(
            checks
                .logs
                .join(format!("artifact-work-{}", std::process::id())),
        )?;
        let artifacts = Artifacts::snapshot(&checks.root, &artifact_fixture.0, &options)?;
        checks.provenance["artifacts"] = artifacts.provenance();
        checks.note(
            "Selected deciding artifacts",
            "PASSED",
            &artifacts.provenance().to_string(),
        );
        let test = checks.cargo(
            &["test", "--workspace", "--all-targets", "--locked"],
            options.offline,
        );
        checks.command(
            "Workspace behavior and installer tests",
            test,
            Duration::from_secs(600),
            &[],
        );
        let extensions = standalone_build(
            &mut checks,
            "examples/extensions/math-guard/Cargo.toml",
            "target/extensions",
            options.offline,
        ) & standalone_build(
            &mut checks,
            "examples/extensions/string-observer/Cargo.toml",
            "target/extensions-observer",
            options.offline,
        );
        if extensions {
            let mut example = checks.cargo(
                &[
                    "run",
                    "--package",
                    "luabuddy-patcher",
                    "--example",
                    "extension_probe",
                    "--locked",
                ],
                options.offline,
            );
            example
                .arg("--")
                .arg(
                    checks
                        .root
                        .join("target/extensions/debug")
                        .join(library("luabuddy_math_guard")),
                )
                .arg(
                    checks
                        .root
                        .join("target/extensions-observer/debug")
                        .join(library("luabuddy_string_observer")),
                );
            checks.command(
                "Compiled Rust extension ABI consumer",
                example,
                Duration::from_secs(60),
                &["PASS"],
            );
        }
        let mut profiles = Command::new(&artifacts.cli.snapshot);
        profiles
            .current_dir(&checks.root)
            .args(["profiles", "--profiles-dir", "profiles"]);
        checks.command(
            "Actual declarative profile catalog",
            profiles,
            Duration::from_secs(30),
            &[],
        );
        let package_fixture = if options.package.is_none() {
            Some(Disposable::new(
                checks
                    .logs
                    .join(format!("package-work-{}", std::process::id())),
            )?)
        } else {
            None
        };
        let package = match &options.package {
            Some(package) => package.canonicalize()?,
            None => package_fixture
                .as_ref()
                .expect("a disposable package has an owned fixture")
                .0
                .join("package"),
        };
        if options.package.is_none() {
            let mut create = Command::new(&artifacts.cli.snapshot);
            create
                .current_dir(&checks.root)
                .args(["package", "--runtime"])
                .arg(&artifacts.runtime.snapshot)
                .args(["--profiles-dir", "profiles", "--out"])
                .arg(&package);
            checks.command(
                "Package current native artifact",
                create,
                Duration::from_secs(60),
                &[],
            );
        }
        if package.is_dir() {
            let mut verify = Command::new(&artifacts.cli.snapshot);
            verify
                .current_dir(&checks.root)
                .args(["verify-package", "--package"])
                .arg(&package);
            let verified = checks
                .command(
                    "Native package integrity",
                    verify,
                    Duration::from_secs(60),
                    &[],
                )
                .is_some();
            if verified {
                match package_matches(&package, &artifacts) {
                    Ok(()) => checks.note(
                        "Package and deciding artifact identity",
                        "PASSED",
                        "Manifest and package runtime/CLI bytes match the exact tested snapshots",
                    ),
                    Err(error) => checks.note(
                        "Package and deciding artifact identity",
                        "FAILED",
                        &error.to_string(),
                    ),
                }
            } else {
                checks.note(
                    "Package and deciding artifact identity",
                    "NOT RUN",
                    "The selected CLI rejected package integrity",
                );
            }
            match package_scan(&package) {
                Ok(()) => checks.note(
                    "Package implementation payloads",
                    "PASSED",
                    "No executable Lua/Java/Python/JS/Wasm payload files or symlinks",
                ),
                Err(error) => checks.note(
                    "Package implementation payloads",
                    "FAILED",
                    &error.to_string(),
                ),
            }
        } else {
            checks.note(
                "Native package integrity",
                "FAILED",
                &format!("Package directory is unavailable: {}", package.display()),
            );
        }
        if let Some(fixture) = package_fixture {
            fixture.remove()?;
        }
        if let Err(error) = native_checks(&mut checks, &options, &artifacts) {
            checks.note("Native game/JVM proofs", "FAILED", &error.to_string());
        }
        artifact_fixture.remove()?;
    } else {
        checks.note(
            "Behavior, artifact and native checks",
            "NOT RUN",
            "The application did not build",
        );
    }
    if options.audit {
        let manifests = [
            "Cargo.lock",
            "examples/extensions/math-guard/Cargo.lock",
            "examples/extensions/string-observer/Cargo.lock",
            "proofs/metadata/Cargo.lock",
            "proofs/runtime/Cargo.lock",
            "proofs/steam/Cargo.lock",
            "proofs/patcher/Cargo.lock",
            "proofs/bridge-loader/Cargo.lock",
            "proofs/activation/Cargo.lock",
            "proofs/extensions/Cargo.lock",
            "proofs/extensions/marker/Cargo.lock",
            "proofs/performance/Cargo.lock",
        ];
        for (index, manifest) in manifests.iter().enumerate() {
            let mut audit = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
            audit
                .current_dir(&checks.root)
                .args(["audit", "--file", manifest]);
            if options.offline || index > 0 {
                audit.arg("--no-fetch");
            }
            if options.offline {
                audit.arg("--no-yanked");
            }
            checks.command(
                &format!("Dependency audit {manifest}"),
                audit,
                Duration::from_secs(180),
                &[],
            );
        }
        if options.offline {
            checks.note(
                "Registry yanked-crate lookup",
                "NOT RUN",
                "Offline advisory audit uses --no-yanked to prevent registry access",
            );
        }
    } else {
        checks.note(
            "Dependency advisory audit",
            "NOT RUN",
            "Pass --audit; unavailable or stale offline databases fail explicitly",
        );
    }
    match inputs(&checks.root, &checks.logs) {
        Ok(after) if after == before => checks.note(
            "Verification source snapshot",
            "PASSED",
            "Workspace inputs remained unchanged during this run",
        ),
        Ok(_) => checks.note(
            "Verification source snapshot",
            "FAILED",
            "Workspace inputs changed during this run; rerun against the final source",
        ),
        Err(error) => checks.note("Verification source snapshot", "FAILED", &error.to_string()),
    }
    checks.finish()
}

fn main() -> ExitCode {
    match Options::parse().and_then(|options| match options {
        None => {
            print!("{HELP}");
            Ok(true)
        }
        Some(options) => run(options),
    }) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("LuaBuddy verification failed: {error}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owned_fixture(name: &str) -> Disposable {
        Disposable::new(env::temp_dir().join(format!(
                "luabuddy-verifier-{name}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            )))
        .unwrap()
    }

    #[test]
    fn selected_artifacts_remain_immutable_when_build_outputs_change() {
        let fixture = owned_fixture("artifacts");
        let runtime = fixture.0.join("runtime-source");
        let cli = fixture.0.join("cli-source");
        fs::write(&runtime, b"abc").unwrap();
        fs::write(&cli, b"hello").unwrap();
        let snapshot = fixture.0.join("snapshot");
        fs::create_dir(&snapshot).unwrap();
        let artifacts = Artifacts::snapshot(
            &fixture.0,
            &snapshot,
            &Options {
                runtime: Some(runtime.clone()),
                cli: Some(cli.clone()),
                ..Options::default()
            },
        )
        .unwrap();
        fs::write(runtime, b"later build output").unwrap();
        fs::write(cli, b"different executable").unwrap();
        assert_eq!(fs::read(&artifacts.runtime.snapshot).unwrap(), b"abc");
        assert_eq!(fs::read(&artifacts.cli.snapshot).unwrap(), b"hello");
        assert_eq!(
            artifacts.runtime.digest,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            artifacts.cli.digest,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn self_consistent_package_must_match_both_deciding_artifacts() {
        let fixture = owned_fixture("package-identity");
        let runtime = fixture.0.join("runtime-source");
        let cli = fixture.0.join("cli-source");
        fs::write(&runtime, b"abc").unwrap();
        fs::write(&cli, b"hello").unwrap();
        let snapshot = fixture.0.join("snapshot");
        fs::create_dir(&snapshot).unwrap();
        let artifacts = Artifacts::snapshot(
            &fixture.0,
            &snapshot,
            &Options {
                runtime: Some(runtime),
                cli: Some(cli),
                ..Options::default()
            },
        )
        .unwrap();
        let package = fixture.0.join("package");
        fs::create_dir(&package).unwrap();
        fs::write(package.join("runtime.so"), b"abc").unwrap();
        fs::write(package.join("luabuddy"), b"hello").unwrap();
        let mut manifest = json!({"os":"linux","runtime":"runtime.so","files":{"runtime.so":{"sha256":"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"},"luabuddy":{"sha256":"2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"}}});
        fs::write(
            package.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        package_matches(&package, &artifacts).unwrap();
        fs::write(package.join("runtime.so"), b"hello").unwrap();
        manifest["files"]["runtime.so"]["sha256"] =
            json!("2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
        fs::write(
            package.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        assert!(
            package_matches(&package, &artifacts)
                .unwrap_err()
                .to_string()
                .contains("runtime.so")
        );
        fs::write(package.join("runtime.so"), b"abc").unwrap();
        manifest["files"]["runtime.so"]["sha256"] =
            json!("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        fs::write(package.join("luabuddy"), b"abc").unwrap();
        manifest["files"]["luabuddy"]["sha256"] =
            json!("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        fs::write(
            package.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        assert!(
            package_matches(&package, &artifacts)
                .unwrap_err()
                .to_string()
                .contains("luabuddy")
        );
    }

    fn fixture_command(mode: &str) -> Command {
        let mut command = Command::new(env::current_exe().expect("test executable exists"));
        command
            .args([
                "--exact",
                "tests::fixture",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("LUABUDDY_VERIFY_FIXTURE", mode);
        command
    }

    #[test]
    fn fixture() {
        let Ok(mode) = env::var("LUABUDDY_VERIFY_FIXTURE") else {
            return;
        };
        match mode.as_str() {
            "passed" => println!("[proof] PASS"),
            "zero-exit-failure" => println!("[proof] PASS\n[proof] FAIL deliberate fixture"),
            "jni-warning" => println!("[proof] PASS\nWARNING in native method: deliberate fixture"),
            "missing-marker" => {
                println!("The fixture returned without proving the requested behavior")
            }
            "descendant-output" => {
                let mut descendant = fixture_command("descendant-output-holder")
                    .spawn()
                    .expect("owned descendant fixture spawns");
                let _exit_direct_child = thread::spawn(|| std::process::exit(0));
                descendant
                    .wait()
                    .expect("owned descendant fixture can be waited on");
            }
            "descendant-output-holder" => thread::sleep(Duration::from_secs(2)),
            "timeout" => {
                if let Some(path) = env::var_os("LUABUDDY_VERIFY_PID_FILE") {
                    fs::write(path, std::process::id().to_string())
                        .expect("owned PID fixture is writable");
                }
                thread::sleep(Duration::from_secs(30));
            }
            _ => std::process::exit(7),
        }
        std::process::exit(0);
    }

    #[test]
    fn proof_failure_or_missing_marker_cannot_pass_with_zero_exit() {
        let passed = capture(&mut fixture_command("passed"), Duration::from_secs(10)).unwrap();
        assert!(proof_clean(&passed, &["[proof] PASS"]));
        for mode in ["zero-exit-failure", "jni-warning", "missing-marker"] {
            let output = capture(&mut fixture_command(mode), Duration::from_secs(10)).unwrap();
            assert!(
                output.success,
                "The {mode} counterexample must actually exit zero"
            );
            assert!(
                !proof_clean(&output, &["[proof] PASS"]),
                "The {mode} proof must be refused"
            );
        }
    }

    #[test]
    fn deadline_stops_and_reaps_the_owned_process() {
        let path = env::temp_dir().join(format!(
            "luabuddy-verifier-timeout-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let fixture = Disposable::new(path).unwrap();
        let pid_file = fixture.0.join("pid");
        let mut command = fixture_command("timeout");
        command.env("LUABUDDY_VERIFY_PID_FILE", &pid_file);
        let started = Instant::now();
        let result = capture(&mut command, Duration::from_secs(1));
        assert!(result.unwrap_err().to_string().contains("deadline"));
        assert!(started.elapsed() < Duration::from_secs(10));
        #[cfg(unix)]
        {
            let pid = fs::read_to_string(pid_file).unwrap();
            let status = Command::new("kill")
                .args(["-0", "--", &pid])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap();
            assert!(
                !status.success(),
                "The owned timed-out process is still alive"
            );
        }
        fixture.remove().unwrap();
    }

    #[test]
    fn deadline_covers_output_retained_after_direct_child_exit() {
        let started = Instant::now();
        let result = capture(
            &mut fixture_command("descendant-output"),
            Duration::from_secs(1),
        );
        assert!(result.unwrap_err().to_string().contains("deadline"));
        assert!(started.elapsed() < Duration::from_secs(10));
    }
}
