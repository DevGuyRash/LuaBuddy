use crate::{
    Result, absolute, bytes, invalid, io_error, optional_bytes, plain_directory, sha256,
    write_atomic, write_new,
};
use luabuddy_patcher::extension::{
    ApprovalRecordV1, EnableConsent, ExtensionManifest, Privilege, verify,
};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

const METADATA_LIMIT: u64 = 1024 * 1024;
const ARTIFACT_LIMIT: u64 = 256 * 1024 * 1024;

pub struct Approval<'a> {
    pub sha256: &'a str,
    pub manifest_sha256: &'a str,
    pub provenance: &'a str,
    pub privileges: BTreeSet<Privilege>,
    pub trust_native_code: bool,
}

fn consent(
    manifest: &ExtensionManifest,
    manifest_sha256: &str,
    approval: &Approval<'_>,
    enabled: bool,
) -> Result<EnableConsent> {
    if !approval.trust_native_code {
        return Err(invalid(
            "Native extensions have the game's full OS privileges. Review the exact binary and supply --trust-native-code before installation or enabling.",
        ));
    }
    if approval.manifest_sha256 != manifest_sha256
        || approval.sha256 != manifest.artifact_sha256
        || approval.provenance != manifest.provenance
        || approval.privileges != manifest.privileges
    {
        return Err(invalid(
            "Approval must match the exact manifest SHA256, provenance, and complete privilege set",
        ));
    }
    Ok(EnableConsent {
        extension_id: manifest.id.clone(),
        artifact_sha256: approval.sha256.to_owned(),
        provenance: approval.provenance.to_owned(),
        allowed_privileges: approval.privileges.clone(),
        install: true,
        enable: enabled,
    })
}

pub fn parse_privileges(value: &str) -> Result<BTreeSet<Privilege>> {
    let mut result = BTreeSet::new();
    for name in value.split(',').filter(|value| !value.is_empty()) {
        let privilege = serde_json::from_value(Value::String(name.to_owned()))
            .map_err(|error| invalid(format!("Unknown extension privilege {name}: {error}")))?;
        if !result.insert(privilege) {
            return Err(invalid(format!("Duplicate privilege {name}")));
        }
    }
    Ok(result)
}

fn store(profiles: &Path) -> Result<PathBuf> {
    Ok(absolute(profiles)?.join("extensions"))
}

fn identity(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() >= 64
        || id
            .bytes()
            .any(|b| !b.is_ascii_alphanumeric() && !b"._-".contains(&b))
        || matches!(id, "." | "..")
        || id == ".luabuddy"
    {
        Err(invalid(
            "Extension ID must be a nonempty portable ABI identifier",
        ))
    } else {
        Ok(())
    }
}

fn artifact_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "artifact.dll"
    } else if cfg!(target_os = "macos") {
        "artifact.dylib"
    } else {
        "artifact.so"
    }
}

fn read_manifest(path: &Path, expected: &str) -> Result<ExtensionManifest> {
    let source = bytes(path, METADATA_LIMIT)?;
    if sha256(&source) != expected {
        return Err(invalid(
            "Manifest bytes differ from the exact native approval",
        ));
    }
    serde_json::from_slice(&source).map_err(|error| {
        invalid(format!(
            "Read extension manifest {}: {error}",
            path.display()
        ))
    })
}

fn read_approval(root: &Path) -> Result<ApprovalRecordV1> {
    let path = root.join("approval.json");
    let record: ApprovalRecordV1 = serde_json::from_slice(&bytes(&path, METADATA_LIMIT)?)
        .map_err(|error| invalid(format!("Read native approval {}: {error}", path.display())))?;
    record
        .validate()
        .map_err(|error| invalid(error.to_string()))?;
    Ok(record)
}

fn owned_generation(root: &Path, approval: &ApprovalRecordV1) -> Result<PathBuf> {
    approval
        .validate()
        .map_err(|error| invalid(error.to_string()))?;
    identity(&approval.consent.extension_id)?;
    Ok(root.join(&approval.manifest_sha256))
}

fn write_immutable(path: &Path, data: &[u8]) -> Result<()> {
    if let Some(existing) = optional_bytes(path, ARTIFACT_LIMIT)? {
        if existing != data {
            return Err(invalid(format!(
                "Preserve changed immutable extension file {}",
                path.display()
            )));
        }
        Ok(())
    } else {
        write_new(path, data, false)
    }
}

fn recover_approval_stage(root: &Path) -> Result<()> {
    let stage = crate::stage_path(&root.join("approval.json"))?;
    if let Some(data) = optional_bytes(&stage, METADATA_LIMIT)? {
        let history = root.join("approval-history");
        plain_directory(&history)?;
        let backup = history.join(format!("interrupted-{}.json", sha256(&data)));
        write_immutable(&backup, &data)?;
        crate::sync_directory(&history)?;
        fs::remove_file(&stage)
            .map_err(|error| io_error("Retire preserved approval stage", &stage, error))?;
        crate::sync_directory(root)?;
    }
    Ok(())
}

pub fn install(
    profiles: &Path,
    artifact: &Path,
    manifest_file: &Path,
    approval: Approval<'_>,
) -> Result<Value> {
    let manifest_bytes = bytes(manifest_file, METADATA_LIMIT)?;
    let manifest_sha256 = sha256(&manifest_bytes);
    let manifest: ExtensionManifest =
        serde_json::from_slice(&manifest_bytes).map_err(|error| invalid(error.to_string()))?;
    identity(&manifest.id)?;
    let checked_consent = consent(&manifest, &manifest_sha256, &approval, true)?;
    let source = bytes(artifact, ARTIFACT_LIMIT)?;
    verify(artifact, manifest.clone(), &checked_consent)
        .map_err(|error| invalid(error.to_string()))?;
    if sha256(&source) != manifest.artifact_sha256 {
        return Err(invalid("Extension changed while preparing installation"));
    }
    let root = store(profiles)?;
    let _lock = crate::InstallLock::acquire(&root)?;
    let directory = root.join(&manifest.id);
    plain_directory(&directory)?;
    recover_approval_stage(&directory)?;
    let generation = directory.join(&manifest_sha256);
    plain_directory(&generation)?;
    write_immutable(&generation.join(artifact_name()), &source)?;
    write_immutable(&generation.join("manifest.json"), &manifest_bytes)?;
    crate::sync_directory(&generation)?;
    let approval_path = directory.join("approval.json");
    let mut next = ApprovalRecordV1 {
        schema_version: 1,
        manifest_sha256: manifest_sha256.clone(),
        consent: checked_consent,
    };
    next.consent.enable = false;
    if let Some(before) = optional_bytes(&approval_path, METADATA_LIMIT)? {
        let prior: ApprovalRecordV1 = serde_json::from_slice(&before).map_err(|error| {
            invalid(format!(
                "Preserve invalid existing native approval: {error}"
            ))
        })?;
        prior
            .validate()
            .map_err(|error| invalid(error.to_string()))?;
        if prior.consent.extension_id != manifest.id {
            return Err(invalid(
                "Existing extension approval belongs to a different identity",
            ));
        }
        if prior.manifest_sha256 == next.manifest_sha256
            && prior.consent.artifact_sha256 == next.consent.artifact_sha256
            && prior.consent.provenance == next.consent.provenance
            && prior.consent.allowed_privileges == next.consent.allowed_privileges
            && prior.consent.install
        {
            return Ok(
                json!({"ok":true,"id":manifest.id,"installed":true,"enabled":prior.consent.enable,"manifestSha256":manifest_sha256,"changed":false,"restartRequired":prior.consent.enable,"effective":"next JVM process"}),
            );
        }
        let history = directory.join("approval-history");
        plain_directory(&history)?;
        let backup = history.join(format!("{}.json", sha256(&before)));
        write_immutable(&backup, &before)?;
        crate::sync_directory(&history)?;
    }
    let encoded = serde_json::to_vec_pretty(&next).map_err(|error| invalid(error.to_string()))?;
    write_atomic(&approval_path, Some(&encoded), false)?;
    Ok(
        json!({"ok":true,"id":manifest.id,"installed":true,"enabled":false,"manifestSha256":manifest_sha256,"changed":true,"store":root,"restartRequired":true,"effective":"next JVM process","message":"Installed with native execution disabled. Enable the exact reviewed artifact and manifest separately."}),
    )
}

pub fn enable(profiles: &Path, id: &str, approval: Approval<'_>) -> Result<Value> {
    identity(id)?;
    let base = store(profiles)?;
    let _lock = crate::InstallLock::acquire(&base)?;
    let root = base.join(id);
    plain_directory(&root)?;
    recover_approval_stage(&root)?;
    let mut prior = read_approval(&root)?;
    if prior.consent.extension_id != id || !prior.consent.install {
        return Err(invalid(
            "No explicit installation approval for this extension",
        ));
    }
    let generation = owned_generation(&root, &prior)?;
    let manifest = read_manifest(&generation.join("manifest.json"), &prior.manifest_sha256)?;
    let next = consent(&manifest, &prior.manifest_sha256, &approval, true)?;
    if next.extension_id != prior.consent.extension_id
        || next.artifact_sha256 != prior.consent.artifact_sha256
    {
        return Err(invalid(
            "Enable approval differs from the installed immutable generation",
        ));
    }
    verify(&generation.join(artifact_name()), manifest, &next)
        .map_err(|error| invalid(error.to_string()))?;
    let changed = !prior.consent.enable;
    prior.consent.enable = true;
    let encoded = serde_json::to_vec_pretty(&prior).map_err(|error| invalid(error.to_string()))?;
    write_atomic(&root.join("approval.json"), Some(&encoded), false)?;
    Ok(
        json!({"ok":true,"id":id,"installed":true,"enabled":true,"manifestSha256":prior.manifest_sha256,"changed":changed,"restartRequired":true,"effective":"next JVM process"}),
    )
}

pub fn disable(profiles: &Path, id: &str) -> Result<Value> {
    identity(id)?;
    let base = store(profiles)?;
    let _lock = crate::InstallLock::acquire(&base)?;
    let root = base.join(id);
    plain_directory(&root)?;
    recover_approval_stage(&root)?;
    let mut approval = read_approval(&root)?;
    if approval.consent.extension_id != id {
        return Err(invalid("Approval identity mismatch"));
    }
    let changed = approval.consent.enable;
    approval.consent.enable = false;
    let encoded =
        serde_json::to_vec_pretty(&approval).map_err(|error| invalid(error.to_string()))?;
    write_atomic(&root.join("approval.json"), Some(&encoded), false)?;
    Ok(
        json!({"ok":true,"id":id,"enabled":false,"changed":changed,"restartRequired":true,"effective":"next JVM process","message":"Existing native code may remain active until the game exits. Restart to apply this saved disable."}),
    )
}

pub fn list(profiles: &Path) -> Result<Value> {
    let base = store(profiles)?;
    let mut extensions = Vec::new();
    let mut failures = 0;
    if base.exists() {
        let mut directories = fs::read_dir(&base)
            .map_err(|error| io_error("Read extension store", &base, error))?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|error| invalid(error.to_string()))?;
        directories.sort();
        for root in directories {
            if !root.is_dir() || root.file_name().and_then(|s| s.to_str()) == Some(".luabuddy") {
                continue;
            }
            let entry = (|| -> Result<Value> {
                let record = read_approval(&root)?;
                let generation = owned_generation(&root, &record)?;
                let manifest =
                    read_manifest(&generation.join("manifest.json"), &record.manifest_sha256)?;
                let approval = record.consent;
                if manifest.id != approval.extension_id
                    || root.file_name().and_then(|name| name.to_str())
                        != Some(approval.extension_id.as_str())
                {
                    return Err(invalid(
                        "Manifest, directory, and native approval identities differ",
                    ));
                }
                let mut validation = approval.clone();
                validation.enable = true;
                let valid = verify(&generation.join(artifact_name()), manifest, &validation)
                    .map_err(|error| invalid(error.to_string()))?;
                let mut generations = Vec::new();
                for entry in fs::read_dir(&root)
                    .map_err(|error| io_error("Read immutable native generations", &root, error))?
                {
                    let entry = entry.map_err(|error| invalid(error.to_string()))?;
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if entry
                        .file_type()
                        .map_err(|error| invalid(error.to_string()))?
                        .is_dir()
                        && name.len() == 64
                        && name
                            .bytes()
                            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
                    {
                        generations.push(name);
                    }
                }
                generations.sort();
                Ok(
                    json!({"id":approval.extension_id,"installed":approval.install,"enabled":approval.enable,"sha256":approval.artifact_sha256,"manifestSha256":record.manifest_sha256,"retainedGenerations":generations,"provenance":approval.provenance,"privileges":approval.allowed_privileges,"patches":valid.manifest().patches,"effective":"next JVM process"}),
                )
            })();
            match entry {
                Ok(entry) => extensions.push(entry),
                Err(error) => {
                    failures += 1;
                    extensions.push(
                        json!({"directory":root,"state":"rejected","error":error.to_string()}),
                    );
                }
            }
        }
    }
    Ok(json!({"ok":failures == 0,"store":base,"extensions":extensions,"nativeCodeExecuted":false}))
}
