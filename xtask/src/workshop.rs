use crate::{Result, file_digest};
use luabuddy::{Package, verify_package};
use serde_json::json;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const ARCHIVE: &str = "LuaBuddy-Setup-linux-x86_64.tar.gz";
const SETUP: &str = "Contents/mods/LuaBuddy_SetupGuide/common/setup";

pub(super) fn run(mut args: impl Iterator<Item = OsString>) -> Result<()> {
    let mut package = None;
    let mut output = None;
    while let Some(arg) = args.next() {
        let slot = match arg.to_str() {
            Some("--package") => &mut package,
            Some("--out") => &mut output,
            Some("--help" | "-h") => {
                println!("cargo xtask workshop --package DIR --out DIR");
                return Ok(());
            }
            _ => return Err(format!("Unknown Workshop staging argument {arg:?}").into()),
        };
        if slot.is_some() {
            return Err(format!("Workshop staging argument {arg:?} was repeated").into());
        }
        *slot = Some(PathBuf::from(args.next().ok_or_else(|| {
            format!("Workshop staging argument {arg:?} requires a directory")
        })?));
    }
    let package = verify_package(&package.ok_or("--package is required")?)?;
    if package.manifest.os != "linux" || package.manifest.architecture != "x86_64" {
        return Err("Graphical Workshop setup currently targets Linux x86_64".into());
    }
    let output = output.ok_or("--out is required")?;
    let output = if output.is_absolute() {
        output
    } else {
        env::current_dir()?.join(output)
    };
    let parent = output
        .parent()
        .ok_or("Output requires a parent directory")?
        .canonicalize()?;
    let output = parent.join(output.file_name().ok_or("Output requires a folder name")?);
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("Verification crate requires a repository parent")?
        .join("workshop")
        .canonicalize()?;
    if output.starts_with(&source) || output.starts_with(&package.directory) {
        return Err("Workshop output must be outside its source guide and native package".into());
    }
    fs::create_dir(&output)?;
    match stage(&package, &source, &output) {
        Ok(archive) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "workshop":output,
                    "packageDigest":package.digest,
                    "packageVersion":package.manifest.version,
                    "archive":archive,
                    "archiveSha256":file_digest(&archive)?,
                    "uploaded":false,
                    "runtimeActivated":false
                }))?
            );
            Ok(())
        }
        Err(error) => {
            fs::remove_dir_all(&output).map_err(|cleanup| {
                format!("{error}; could not remove owned failed output: {cleanup}")
            })?;
            Err(error)
        }
    }
}

fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    if metadata.is_dir() {
        fs::create_dir(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_tree(&entry.path(), &destination.join(entry.file_name()))?;
        }
    } else if metadata.is_file() {
        fs::copy(source, destination)?;
    } else {
        return Err(format!(
            "Workshop source must contain regular files: {}",
            source.display()
        )
        .into());
    }
    Ok(())
}

fn package_snapshot(package: &Package, root: &Path) -> Result<()> {
    fs::create_dir(root)?;
    for (name, file) in &package.manifest.files {
        let target = root.join(name);
        fs::create_dir_all(target.parent().ok_or("Package file requires a parent")?)?;
        fs::copy(package.directory.join(name), &target)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                &target,
                fs::Permissions::from_mode(if file.executable { 0o755 } else { 0o644 }),
            )?;
        }
        #[cfg(not(unix))]
        let _ = file;
    }
    for name in ["manifest.json", "manifest.sha256"] {
        fs::copy(package.directory.join(name), root.join(name))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(root.join(name), fs::Permissions::from_mode(0o644))?;
        }
    }
    let snapshot = verify_package(root)?;
    if snapshot.digest != package.digest {
        return Err("Package changed while preparing its Workshop archive".into());
    }
    Ok(())
}

fn stage(package: &Package, source: &Path, output: &Path) -> Result<PathBuf> {
    for name in ["Contents", "workshop.txt", "preview.png"] {
        copy_tree(&source.join(name), &output.join(name))?;
    }
    let setup = output.join(SETUP);
    if setup.exists() {
        return Err("Tracked guide must not contain a generated setup payload".into());
    }
    fs::create_dir(&setup)?;
    let archive = setup.join(ARCHIVE);
    let scratch = output.join(".native-package");
    fs::create_dir(&scratch)?;
    let folder = format!(
        "LuaBuddy-{}",
        package.digest.chars().take(12).collect::<String>()
    );
    package_snapshot(package, &scratch.join(&folder))?;
    let status = Command::new("tar")
        .args([
            "--format=ustar",
            "--sort=name",
            "--mtime=@0",
            "--owner=0",
            "--group=0",
            "--numeric-owner",
            "--mode=u=rwX,go=rX",
            "-czf",
        ])
        .arg(&archive)
        .arg("-C")
        .arg(&scratch)
        .arg(&folder)
        .status()?;
    if !status.success() {
        return Err("GNU tar could not create the Workshop setup archive".into());
    }
    fs::remove_dir_all(scratch)?;
    fs::write(
        setup.join(format!("{ARCHIVE}.sha256")),
        format!("{}  {ARCHIVE}\n", file_digest(&archive)?),
    )?;
    Ok(archive)
}

#[cfg(all(test, target_os = "linux", target_arch = "x86_64"))]
mod tests {
    use super::*;
    use crate::Disposable;
    use std::collections::BTreeSet;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fixture() -> (Disposable, Package) {
        let root = std::env::temp_dir().join(format!(
            "luabuddy-workshop-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let owned = Disposable::new(root).unwrap();
        let image = fs::read(env::current_exe().unwrap()).unwrap();
        let runtime = owned.0.join("runtime.so");
        fs::write(&runtime, &image[..64]).unwrap();
        let templates = owned.0.join("templates");
        fs::create_dir(&templates).unwrap();
        fs::write(templates.join("Example.properties"), "id=WorkshopFixture\n").unwrap();
        let package =
            luabuddy::make_package(&runtime, &templates, &owned.0.join("package")).unwrap();
        (owned, package)
    }

    #[test]
    fn workshop_archive_is_repeatable_and_graphically_executable() {
        let (owned, package) = fixture();
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("workshop");
        let first = owned.0.join("first");
        let second = owned.0.join("second");
        fs::create_dir(&first).unwrap();
        fs::create_dir(&second).unwrap();
        let archive = stage(&package, &source, &first).unwrap();
        let repeated = stage(&package, &source, &second).unwrap();
        assert_eq!(fs::read(&archive).unwrap(), fs::read(repeated).unwrap());
        let names = fs::read_dir(&first)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            names,
            ["Contents", "preview.png", "workshop.txt"]
                .map(OsString::from)
                .into_iter()
                .collect()
        );
        assert!(
            Command::new("sha256sum")
                .current_dir(archive.parent().unwrap())
                .args(["--check", &format!("{ARCHIVE}.sha256")])
                .status()
                .unwrap()
                .success()
        );
        let extracted = owned.0.join("extracted");
        fs::create_dir(&extracted).unwrap();
        assert!(
            Command::new("tar")
                .arg("-xzf")
                .arg(archive)
                .arg("-C")
                .arg(&extracted)
                .status()
                .unwrap()
                .success()
        );
        let folders = fs::read_dir(&extracted)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(folders.len(), 1);
        let restored = verify_package(&folders[0]).unwrap();
        assert_eq!(restored.digest, package.digest);
        assert_eq!(
            fs::metadata(folders[0].join("luabuddy"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        assert_eq!(
            fs::metadata(folders[0].join("manifest.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o644
        );
    }

    #[test]
    fn workshop_snapshot_refuses_package_changes_and_source_symlinks() {
        let (owned, package) = fixture();
        fs::write(
            package.directory.join("profiles/Example.properties"),
            "id=ChangedAfterVerification\n",
        )
        .unwrap();
        assert!(package_snapshot(&package, &owned.0.join("snapshot")).is_err());
        let link = owned.0.join("guide-link");
        symlink(package.directory.join("LICENSE"), &link).unwrap();
        let destination = owned.0.join("copied-link");
        assert!(copy_tree(&link, &destination).is_err());
        assert!(!destination.exists());
    }
}
