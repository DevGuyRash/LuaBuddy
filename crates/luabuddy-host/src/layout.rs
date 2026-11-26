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

#[cfg(test)]
mod tests {
    use super::approved;

    #[test]
    fn only_the_recorded_bytecode_is_approved() {
        // The fingerprint layouts.json records for Event.trigger in 42.22.2.
        let event = "(Lse/krka/kahlua/vm/KahluaTable;Lse/krka/kahlua/integration/LuaCaller;[Ljava/lang/Object;)Z";
        let recorded = "879d99fb2c957fcf4aa6544cdfbadec79fda95eb9157f69380f490845d72cd11";
        assert_eq!(
            approved("zombie/Lua/Event", "trigger", event, recorded),
            Ok(true)
        );
        assert_eq!(
            approved("zombie/Lua/Event", "trigger", event, &"0".repeat(64)),
            Ok(false)
        );
        assert_eq!(
            approved("zombie/Lua/Event", "call", event, recorded),
            Ok(false)
        );
    }
}
