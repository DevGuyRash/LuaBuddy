use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs, io,
    path::{Path, PathBuf},
    process::{Command, ExitCode, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

struct OwnedDirectory(PathBuf);

impl OwnedDirectory {
    fn create(path: PathBuf) -> io::Result<Self> {
        fs::create_dir(&path)?;
        Ok(Self(path))
    }
}

impl Drop for OwnedDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Options {
    source: PathBuf,
    out: PathBuf,
    evidence: PathBuf,
    epoch: u64,
}

struct Toolchain {
    cargo: PathBuf,
    rustc: PathBuf,
    version: String,
}

fn toolchain(source: &Path) -> Result<Toolchain> {
    let declaration = fs::read_to_string(source.join("rust-toolchain.toml"))?;
    let channel = declaration
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("channel = ")
                .map(|value| value.trim_matches('"'))
        })
        .ok_or("Pinned Rust channel missing")?;
    let sysroot = PathBuf::from(tool("rustc", &["--print", "sysroot"])?);
    let cargo = sysroot.join("bin/cargo").canonicalize()?;
    let rustc = sysroot.join("bin/rustc").canonicalize()?;
    for executable in [&cargo, &rustc] {
        if !fs::read(executable)?.starts_with(b"\x7fELF") {
            return Err(format!(
                "Pinned tool is not a native ELF executable: {}",
                executable.display()
            )
            .into());
        }
    }
    let output = Command::new(&rustc).arg("--version").output()?;
    let version = String::from_utf8(output.stdout)?.trim().to_owned();
    if !output.status.success() || !version.starts_with(&format!("rustc {channel} ")) {
        return Err("The direct compiler does not match rust-toolchain.toml".into());
    }
    Ok(Toolchain {
        cargo,
        rustc,
        version,
    })
}

impl Options {
    fn parse() -> Result<Self> {
        let mut values = BTreeMap::new();
        let mut args = env::args_os().skip(1);
        while let Some(key) = args.next() {
            let key = key.into_string().map_err(|_| "Arguments must be UTF-8")?;
            if key == "--help" {
                println!(
                    "LuaBuddy reproducible release proof\n\n  cargo run --manifest-path proofs/reproducible/Cargo.toml -- --source DIR --out DIR --evidence DIR [--epoch UNIX]\n\nBuilds the two production packages offline from distinct source paths and target directories, compares complete artifact and package bytes, then compares normalized archives. No game files are copied or executed."
                );
                std::process::exit(0);
            }
            if !matches!(
                key.as_str(),
                "--source" | "--out" | "--evidence" | "--epoch"
            ) {
                return Err(format!("Unknown option {key}").into());
            }
            if values
                .insert(
                    key.clone(),
                    args.next().ok_or_else(|| format!("{key} needs a value"))?,
                )
                .is_some()
            {
                return Err(format!("Repeated option {key}").into());
            }
        }
        let source = values
            .get("--source")
            .map(PathBuf::from)
            .unwrap_or(env::current_dir()?)
            .canonicalize()?;
        let out = values
            .get("--out")
            .map(PathBuf::from)
            .unwrap_or_else(|| source.join("dist/reproducibility"));
        let evidence = PathBuf::from(
            values
                .get("--evidence")
                .ok_or("Provide --evidence outside the source tree")?,
        );
        fs::create_dir_all(&out)?;
        fs::create_dir_all(&evidence)?;
        let out = out.canonicalize()?;
        let evidence = evidence.canonicalize()?;
        if evidence.starts_with(&source) {
            return Err("Evidence must remain outside the source tree".into());
        }
        let epoch = match values.get("--epoch") {
            Some(value) => value.to_str().ok_or("Epoch must be UTF-8")?.parse()?,
            None => String::from_utf8(
                Command::new("git")
                    .args(["log", "-1", "--format=%ct"])
                    .current_dir(&source)
                    .output()?
                    .stdout,
            )?
            .trim()
            .parse()?,
        };
        Ok(Self {
            source,
            out,
            evidence,
            epoch,
        })
    }
}

#[derive(Clone)]
struct Input {
    relative: PathBuf,
    bytes: Vec<u8>,
    mode: u32,
}

fn declaration_or_catalog(path: &Path) -> bool {
    let text = path.to_string_lossy();
    matches!(
        text.as_ref(),
        "Cargo.toml"
            | "Cargo.lock"
            | "rust-toolchain.toml"
            | ".cargo/config.toml"
            | "LICENSE"
            | "THIRD_PARTY.md"
    ) || text.starts_with("profiles/")
        && (text.ends_with(".properties") || text == "profiles/migration.json")
        || text.starts_with("crates/") && text.ends_with("Cargo.toml")
}

fn source_candidate(path: &Path) -> bool {
    let text = path.to_string_lossy();
    declaration_or_catalog(path)
        || text.starts_with("crates/")
        || text.starts_with("profiles/")
        || text.starts_with("licenses/")
        || text.starts_with("proofs/")
        || text.starts_with("xtask/")
}

fn source_files(source: &Path) -> Result<Vec<PathBuf>> {
    let output = Command::new("git")
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .current_dir(source)
        .output()?;
    if !output.status.success() {
        return Err("Git could not enumerate the source checkout".into());
    }
    let text = String::from_utf8(output.stdout)?;
    let mut result = Vec::new();
    for name in text.split('\0').filter(|name| !name.is_empty()) {
        let path = PathBuf::from(name);
        if !source_candidate(&path) {
            continue;
        }
        let full = source.join(&path);
        match fs::symlink_metadata(&full) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(
                    format!("Source inventory contains a symlink: {}", path.display()).into(),
                );
            }
            Ok(metadata) if metadata.is_file() => result.push(path),
            Ok(_) => (),
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
    }
    result.sort();
    result.dedup();
    Ok(result)
}

fn snapshot(source: &Path, files: &[PathBuf]) -> Result<Vec<Input>> {
    files
        .iter()
        .map(|relative| {
            let path = source.join(relative);
            let metadata = fs::symlink_metadata(&path)?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(
                    format!("Expected a regular source file: {}", relative.display()).into(),
                );
            }
            #[cfg(unix)]
            let mode = {
                use std::os::unix::fs::PermissionsExt;
                metadata.permissions().mode() & 0o777
            };
            #[cfg(not(unix))]
            let mode = 0o644;
            Ok(Input {
                relative: relative.clone(),
                bytes: fs::read(path)?,
                mode,
            })
        })
        .collect()
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn input_hash(inputs: &[Input], selected: &BTreeSet<PathBuf>) -> Result<String> {
    let mut hash = Sha256::new();
    let lookup: BTreeMap<_, _> = inputs
        .iter()
        .map(|input| (&input.relative, input))
        .collect();
    for path in selected {
        let input = lookup.get(path).ok_or_else(|| {
            format!(
                "Compiled input was absent from the initial snapshot: {}",
                path.display()
            )
        })?;
        hash.update(input.relative.to_string_lossy().as_bytes());
        hash.update([0]);
        hash.update(input.mode.to_le_bytes());
        hash.update((input.bytes.len() as u64).to_le_bytes());
        hash.update(&input.bytes);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn dep_words(text: &str) -> Result<Vec<String>> {
    let mut result = Vec::new();
    let mut word = String::new();
    let mut escaped = false;
    for ch in text.chars() {
        if escaped {
            if ch != '\n' {
                word.push(ch);
            }
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch.is_whitespace() {
            if !word.is_empty() {
                result.push(std::mem::take(&mut word));
            }
        } else {
            word.push(ch);
        }
    }
    if escaped {
        return Err("Truncated rustc dependency escape".into());
    }
    if !word.is_empty() {
        result.push(word);
    }
    Ok(result)
}

fn compiled_inputs(
    source: &Path,
    target: &Path,
    inventory: &[PathBuf],
) -> Result<BTreeSet<PathBuf>> {
    let mut result: BTreeSet<_> = inventory
        .iter()
        .filter(|path| declaration_or_catalog(path))
        .cloned()
        .collect();
    for name in ["libluabuddy_host.d", "luabuddy.d"] {
        let text = fs::read_to_string(target.join("release").join(name))?;
        for line in text.lines().filter(|line| !line.starts_with('#')) {
            let Some((_, dependencies)) = line.split_once(": ") else {
                continue;
            };
            for word in dep_words(dependencies)? {
                let path = PathBuf::from(word);
                let path = if path.is_absolute() {
                    path
                } else {
                    source.join(path)
                }
                .canonicalize()?;
                let relative = path.strip_prefix(source).map_err(|_| {
                    format!(
                        "First-party dependency is outside its source checkout: {}",
                        path.display()
                    )
                })?;
                result.insert(relative.to_owned());
            }
        }
    }
    if !result.contains(Path::new("crates/luabuddy-host/src/lib.rs"))
        || !result.contains(Path::new("crates/luabuddy-cli/src/main.rs"))
    {
        return Err(
            "Actual rustc dependency files omit the expected production entrypoints".into(),
        );
    }
    Ok(result)
}

fn copy_source(inputs: &[Input], selected: &BTreeSet<PathBuf>, destination: &Path) -> Result<()> {
    fs::create_dir(destination)?;
    for input in inputs {
        let name = input.relative.to_string_lossy();
        if !selected.contains(&input.relative)
            && !name.starts_with("crates/")
            && !name.starts_with("xtask/")
        {
            continue;
        }
        let target = destination.join(&input.relative);
        fs::create_dir_all(target.parent().ok_or("Source file has no parent")?)?;
        fs::write(&target, &input.bytes)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&target, fs::Permissions::from_mode(input.mode))?;
        }
    }
    Ok(())
}

fn run(mut command: Command, label: &str, log: &Path, deadline: Duration) -> Result<()> {
    let output = fs::File::create(log)?;
    command
        .stdout(Stdio::from(output.try_clone()?))
        .stderr(Stdio::from(output));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn()?;
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                return Err(format!("{label} failed with {status}; see {}", log.display()).into());
            }
            println!("PASS {label}");
            return Ok(());
        }
        if started.elapsed() >= deadline {
            #[cfg(unix)]
            {
                let _ = Command::new("kill")
                    .args(["-TERM", "--", &format!("-{}", child.id())])
                    .status();
                thread::sleep(Duration::from_millis(300));
                let _ = Command::new("kill")
                    .args(["-KILL", "--", &format!("-{}", child.id())])
                    .status();
            }
            #[cfg(not(unix))]
            {
                let _ = child.kill();
            }
            let _ = child.wait();
            return Err(format!(
                "{label} exceeded its deadline; its owned process group was stopped"
            )
            .into());
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn fresh_codegen(log: &Path, tools: &Toolchain) -> Result<()> {
    let text = fs::read_to_string(log)?;
    for (name, source) in [
        ("luabuddy_host", "crates/luabuddy-host/src/lib.rs"),
        ("luabuddy", "crates/luabuddy-cli/src/main.rs"),
    ] {
        if !text.lines().any(|line| {
            line.contains("Running `")
                && line.contains(&format!("{} --crate-name {name}", tools.rustc.display()))
                && line.contains(source)
                && line.contains("--emit=dep-info,link")
                && !line.contains("-C incremental=")
        }) {
            return Err(
                format!("No direct fresh production codegen/link command for {source}").into(),
            );
        }
    }
    Ok(())
}

fn cargo_command(
    source: &Path,
    target: &Path,
    epoch: u64,
    registry: &Path,
    tools: &Toolchain,
) -> Command {
    let mut command = Command::new(&tools.cargo);
    command
        .current_dir(source)
        .args([
            "build",
            "-vv",
            "--jobs=4",
            "--release",
            "--locked",
            "--offline",
            "-p",
            "luabuddy",
            "-p",
            "luabuddy-host",
        ])
        .env("CARGO_TARGET_DIR", target)
        .env("CARGO_INCREMENTAL", "0")
        .env("SOURCE_DATE_EPOCH", epoch.to_string())
        .arg("--config")
        .arg("build.rustc-wrapper=\"\"")
        .arg("--config")
        .arg("build.rustc-workspace-wrapper=\"\"")
        .env("RUSTC", &tools.rustc)
        .env(
            "CARGO_ENCODED_RUSTFLAGS",
            [
                format!("--remap-path-prefix={}=luabuddy", source.display()),
                format!("--remap-path-prefix={}=cargo-registry", registry.display()),
                format!("--remap-path-prefix={}=build-output", target.display()),
            ]
            .join("\u{1f}"),
        )
        .env_remove("RUSTFLAGS")
        .env_remove("RUSTC_WRAPPER")
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .env_remove("CARGO_BUILD_RUSTC_WRAPPER")
        .env_remove("CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER")
        .env_remove("CARGO_BUILD_TARGET")
        .env_remove("CARGO_MAKEFLAGS")
        .env_remove("MAKEFLAGS")
        .env_remove("MFLAGS")
        .env("SCCACHE_DISABLE", "1")
        .env("CCACHE_DISABLE", "1")
        .env("TZ", "UTC")
        .env("LANG", "C.UTF-8")
        .env("LC_ALL", "C.UTF-8");
    command
}

fn package(cli: &Path, runtime: &Path, catalog: &Path, output: &Path, log: &Path) -> Result<()> {
    let mut command = Command::new(cli);
    command
        .args(["package", "--runtime"])
        .arg(runtime)
        .arg("--profiles-dir")
        .arg(catalog)
        .arg("--out")
        .arg(output);
    run(
        command,
        "native package creation",
        log,
        Duration::from_secs(120),
    )?;
    let mut command = Command::new(cli);
    command.arg("verify-package").arg("--package").arg(output);
    run(
        command,
        "native package integrity",
        &log.with_extension("verify.log"),
        Duration::from_secs(120),
    )
}

fn files(root: &Path) -> Result<BTreeMap<PathBuf, Vec<u8>>> {
    let mut pending = vec![root.to_owned()];
    let mut result = BTreeMap::new();
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                return Err("A package contains a symlink".into());
            }
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                result.insert(
                    entry.path().strip_prefix(root)?.to_owned(),
                    fs::read(entry.path())?,
                );
            } else {
                return Err("A package contains an unsupported file type".into());
            }
        }
    }
    Ok(result)
}

fn archive(package: &Path, output: &Path, epoch: u64, log: &Path) -> Result<()> {
    let mut command = Command::new("tar");
    command
        .args([
            "--sort=name",
            "--format=ustar",
            "--owner=0",
            "--group=0",
            "--numeric-owner",
            "--mode=go-w,u+rwX,go+rX",
        ])
        .arg(format!("--mtime=@{epoch}"))
        .args(["-I", "gzip -n", "-cf"])
        .arg(output)
        .arg("-C")
        .arg(package.parent().ok_or("Package has no parent")?)
        .arg(package.file_name().ok_or("Package has no basename")?);
    run(
        command,
        "normalized release archive",
        log,
        Duration::from_secs(120),
    )
}

fn tool(program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program).args(args).output()?;
    if !output.status.success() {
        return Err(format!("Could not inspect {program}").into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

fn execute(options: Options) -> Result<()> {
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        return Err("This release proof currently covers Linux x86_64".into());
    }
    let inventory = source_files(&options.source)?;
    let before = snapshot(&options.source, &inventory)?;
    let tools = toolchain(&options.source)?;
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let scratch = OwnedDirectory::create(
        env::temp_dir().join(format!("luabuddy-repro-{}-{nonce}", std::process::id())),
    )?;
    let relocated = scratch.0.join("different-checkout");
    let registry = env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")))
        .ok_or("Cargo cache directory unavailable")?
        .join("registry/src")
        .canonicalize()?;
    let targets = options.source.join("target").join(format!("repro-{nonce}"));
    fs::create_dir_all(options.source.join("target"))?;
    fs::create_dir(&targets)?;
    let target_a = targets.join("a");
    let target_b = targets.join("b");
    run(
        cargo_command(&options.source, &target_a, options.epoch, &registry, &tools),
        "release build A",
        &options.evidence.join("build-a.log"),
        Duration::from_secs(900),
    )?;
    fresh_codegen(&options.evidence.join("build-a.log"), &tools)?;
    let selected = compiled_inputs(&options.source, &target_a, &inventory)?;
    let source_hash = input_hash(&before, &selected)?;
    let current_inventory = source_files(&options.source)?;
    let current = snapshot(&options.source, &current_inventory)?;
    if compiled_inputs(&options.source, &target_a, &current_inventory)? != selected
        || input_hash(&current, &selected)? != source_hash
    {
        return Err(
            "Production source inputs changed during build A; no deciding artifact accepted".into(),
        );
    }
    copy_source(&before, &selected, &relocated)?;
    let release_a = target_a.join("release");
    let runtime_a = release_a.join("libluabuddy_host.so");
    let cli_a = release_a.join("luabuddy");
    let native = fs::read(&runtime_a)?;
    let cli = fs::read(&cli_a)?;
    for prefix in [&options.source, &registry] {
        let bytes = prefix.to_string_lossy();
        if native
            .windows(bytes.len())
            .any(|part| part == bytes.as_bytes())
            || cli
                .windows(bytes.len())
                .any(|part| part == bytes.as_bytes())
        {
            return Err("A release artifact retains an absolute source path".into());
        }
    }
    let run_root = options
        .out
        .join(format!("run-{}-{nonce}", &source_hash[..12]));
    fs::create_dir(&run_root)?;
    let candidate = run_root.join("candidate");
    fs::create_dir(&candidate)?;
    fs::copy(&runtime_a, candidate.join("libluabuddy_host.so"))?;
    fs::copy(&cli_a, candidate.join("luabuddy"))?;
    let candidate_record = json!({"schema":1,"sourceInputsSha256":source_hash,"runtimeSha256":digest(&native),"cliSha256":digest(&cli),"candidate":candidate,"reproducible":false});
    fs::write(
        options.evidence.join("candidate.json"),
        serde_json::to_vec_pretty(&candidate_record)?,
    )?;
    println!(
        "CANDIDATE {} runtimeSHA256={} cliSHA256={}",
        candidate.display(),
        digest(&native),
        digest(&cli)
    );
    run(
        cargo_command(&relocated, &target_b, options.epoch, &registry, &tools),
        "release build B from relocated source",
        &options.evidence.join("build-b.log"),
        Duration::from_secs(900),
    )?;
    fresh_codegen(&options.evidence.join("build-b.log"), &tools)?;
    let selected_b = compiled_inputs(&relocated, &target_b, &inventory)?;
    if selected_b != selected {
        return Err("Relocated build used a different production input graph".into());
    }
    let selected_files: Vec<_> = selected.iter().cloned().collect();
    if input_hash(&snapshot(&relocated, &selected_files)?, &selected)? != source_hash {
        return Err(
            "The relocated compiled source inputs differ from the original snapshot".into(),
        );
    }
    let release_b = target_b.join("release");
    let native_b = fs::read(release_b.join("libluabuddy_host.so"))?;
    let cli_b = fs::read(release_b.join("luabuddy"))?;
    if native != native_b || cli != cli_b {
        let mismatch = json!({"sourceInputsSha256":source_hash,"runtimeA":digest(&native),"runtimeB":digest(&native_b),"cliA":digest(&cli),"cliB":digest(&cli_b),"runtimeEqual":native==native_b,"cliEqual":cli==cli_b});
        fs::write(
            options.evidence.join("mismatch.json"),
            serde_json::to_vec_pretty(&mismatch)?,
        )?;
        return Err("Fresh relocated release artifacts differ; see mismatch.json".into());
    }
    let a_parent = run_root.join("a");
    let b_parent = run_root.join("b");
    fs::create_dir(&a_parent)?;
    fs::create_dir(&b_parent)?;
    let a = a_parent.join("LuaBuddy-linux-x86_64");
    let b = b_parent.join("LuaBuddy-linux-x86_64");
    package(
        &cli_a,
        &runtime_a,
        &options.source.join("profiles"),
        &a,
        &options.evidence.join("package-a.log"),
    )?;
    package(
        &release_b.join("luabuddy"),
        &release_b.join("libluabuddy_host.so"),
        &relocated.join("profiles"),
        &b,
        &options.evidence.join("package-b.log"),
    )?;
    let package_a = files(&a)?;
    let package_b = files(&b)?;
    if package_a != package_b {
        return Err("Packages from independent release builds differ".into());
    }
    let archive_a = run_root.join("LuaBuddy-linux-x86_64.tar.gz");
    let archive_b = options.evidence.join("independent-package.tar.gz");
    archive(
        &a,
        &archive_a,
        options.epoch,
        &options.evidence.join("archive-a.log"),
    )?;
    archive(
        &b,
        &archive_b,
        options.epoch,
        &options.evidence.join("archive-b.log"),
    )?;
    let archive_bytes = fs::read(&archive_a)?;
    if archive_bytes != fs::read(&archive_b)? {
        return Err("Normalized release archives differ".into());
    }
    let final_inventory = source_files(&options.source)?;
    let final_inputs = snapshot(&options.source, &final_inventory)?;
    if compiled_inputs(&options.source, &target_a, &final_inventory)? != selected
        || input_hash(&final_inputs, &selected)? != source_hash
    {
        return Err(
            "Production source inputs changed during verification; release acceptance invalidated"
                .into(),
        );
    }
    let files = package_a
        .iter()
        .map(|(path, bytes)| {
            (
                path.to_string_lossy().into_owned(),
                json!({"sha256":digest(bytes),"size":bytes.len()}),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let report: Value = json!({"schema":1,"reproducible":true,"scope":"Linux x86_64, same recorded build environment, distinct source and target paths","sourceInputsSha256":source_hash,"sourceInputFiles":selected,"sourceDateEpoch":options.epoch,"rustc":tools.version,"compilerSha256":digest(&fs::read(&tools.rustc)?),"cargoSha256":digest(&fs::read(&tools.cargo)?),"directToolchainExecutables":true,"directCodegenCommandsVerified":true,"compilerWrappersDisabled":true,"freshTargetDirectories":true,"tar":tool("tar", &["--version"])? ,"gzip":tool("gzip", &["--version"])? ,"runtimeSha256":digest(&native),"cliSha256":digest(&cli),"archiveSha256":digest(&archive_bytes),"packageFiles":files,"nativeRuntimeAcceptance":"Requires separate actual game/JVM verification","otherPlatforms":"Not run"});
    fs::write(
        run_root.join("build-provenance.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    fs::write(
        options.evidence.join("reproducible.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    fs::write(
        run_root.join("SHA256SUMS"),
        format!("{}  LuaBuddy-linux-x86_64.tar.gz\n", digest(&archive_bytes)),
    )?;
    fs::remove_dir_all(&b_parent)?;
    println!(
        "PASS byte-identical runtime, CLI, {} package files, and normalized archive from independent source builds",
        package_a.len()
    );
    println!("RELEASE {}", run_root.display());
    Ok(())
}

fn main() -> ExitCode {
    match Options::parse().and_then(execute) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("FAIL reproducible release proof: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owned() -> OwnedDirectory {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        OwnedDirectory::create(env::temp_dir().join(format!(
            "luabuddy-repro-test-{}-{nonce}",
            std::process::id()
        )))
        .unwrap()
    }

    fn write(root: &Path, name: &str, bytes: &[u8]) {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn existing_directory_refusal_preserves_independent_data() {
        let parent = owned();
        let existing = parent.0.join("preexisting");
        fs::create_dir(&existing).unwrap();
        fs::write(existing.join("sentinel"), b"keep this").unwrap();
        assert!(OwnedDirectory::create(existing.clone()).is_err());
        assert_eq!(fs::read(existing.join("sentinel")).unwrap(), b"keep this");
    }

    #[test]
    fn dependency_paths_preserve_escaped_spaces_and_backslashes() {
        assert_eq!(
            dep_words("/tmp/source\\ path/main.rs /tmp/back\\\\slash.rs").unwrap(),
            ["/tmp/source path/main.rs", "/tmp/back\\slash.rs"]
        );
        assert!(dep_words("/tmp/truncated\\").is_err());
    }

    #[test]
    fn actual_dependency_graph_controls_input_invalidation() {
        let root = owned();
        let target = root.0.join("target");
        let names = [
            "Cargo.toml",
            "crates/luabuddy-host/src/lib.rs",
            "crates/luabuddy-cli/src/main.rs",
            "crates/luabuddy-host/src/runtime_proof.rs",
            "proofs/patcher/src/scenario.rs",
        ];
        for name in names {
            write(&root.0, name, name.as_bytes());
        }
        let dependencies = format!(
            "artifact: {} {} {}\n",
            root.0.join(names[1]).display(),
            root.0.join(names[2]).display(),
            root.0.join(names[4]).display()
        );
        write(
            &target,
            "release/libluabuddy_host.d",
            dependencies.as_bytes(),
        );
        write(&target, "release/luabuddy.d", dependencies.as_bytes());
        let inventory = names.into_iter().map(PathBuf::from).collect::<Vec<_>>();
        let selected = compiled_inputs(&root.0, &target, &inventory).unwrap();
        assert!(selected.contains(Path::new(names[4])));
        assert!(!selected.contains(Path::new(names[3])));
        let before = input_hash(&snapshot(&root.0, &inventory).unwrap(), &selected).unwrap();
        write(&root.0, names[3], b"updated uncompiled driver");
        assert_eq!(
            before,
            input_hash(&snapshot(&root.0, &inventory).unwrap(), &selected).unwrap()
        );
        write(&root.0, names[4], b"updated embedded agent mode");
        assert_ne!(
            before,
            input_hash(&snapshot(&root.0, &inventory).unwrap(), &selected).unwrap()
        );
    }
}
