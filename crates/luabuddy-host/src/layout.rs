//! The game methods LuaBuddy was verified against, identified by bytecode.

use sha2::{Digest, Sha256};

/// The fingerprint `layouts.json` records for a method's bytecode.
pub(crate) fn fingerprint(bytecodes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytecodes))
}

/// Whether this exact method, with this fingerprint, is an approved target.
pub(crate) fn approved(
    class: &str,
    method: &str,
    descriptor: &str,
    fingerprint: &str,
) -> Result<bool, String> {
    let layout: serde_json::Value = serde_json::from_str(include_str!("../layouts.json"))
        .map_err(|e| format!("invalid host layout manifest: {e}"))?;
    if layout["schema"] != 1 || layout["class_loader"] != "system" {
        return Err("unsupported host layout manifest schema".into());
    }
    Ok(layout["targets"]
        .as_array()
        .ok_or("missing host target manifest")?
        .iter()
        .any(|target| {
            target["class"] == class
                && target["method"] == method
                && target["descriptor"] == descriptor
                && target["bytecode_sha256"] == fingerprint
        }))
}
