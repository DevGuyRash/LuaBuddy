use crate::*;
use serde_json::{Value, json};
use std::path::Path;

pub(crate) fn valid_utc_timestamp(value: &str) -> bool {
    let body = value
        .strip_suffix('Z')
        .or_else(|| value.strip_suffix("+00:00"));
    let Some(body) = body else {
        return false;
    };
    let Some(base) = body.as_bytes().get(..19) else {
        return false;
    };
    for (index, byte) in base.iter().enumerate() {
        let expected = match index {
            4 | 7 => Some(b'-'),
            10 => Some(b'T'),
            13 | 16 => Some(b':'),
            _ => None,
        };
        if expected.map_or_else(|| !byte.is_ascii_digit(), |expected| *byte != expected) {
            return false;
        }
    }
    let fraction = &body[19..];
    if !fraction.is_empty()
        && (!fraction.starts_with('.')
            || !(2..=10).contains(&fraction.len())
            || !fraction[1..].bytes().all(|b| b.is_ascii_digit()))
    {
        return false;
    }
    let number = |start: usize, end: usize| body[start..end].parse::<u32>().ok();
    let (Some(year), Some(month), Some(day), Some(hour), Some(minute), Some(second)) = (
        number(0, 4),
        number(5, 7),
        number(8, 10),
        number(11, 13),
        number(14, 16),
        number(17, 19),
    ) else {
        return false;
    };
    if year == 0 || !(1..=12).contains(&month) || hour > 23 || minute > 59 || second > 59 {
        return false;
    }
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let maximum_day = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    (1..=maximum_day).contains(&day)
}

pub(crate) fn valid_session(value: &str) -> bool {
    let Some(uuid) = value.as_bytes().get(..36) else {
        return false;
    };
    for (index, byte) in uuid.iter().enumerate() {
        if [8, 13, 18, 23].contains(&index) {
            if *byte != b'-' {
                return false;
            }
        } else if !byte.is_ascii_hexdigit() {
            return false;
        }
    }
    value.len() == 36
        || value.get(36..).is_some_and(|suffix| {
            suffix.strip_prefix('-').is_some_and(|count| {
                !count.is_empty() && count.bytes().all(|byte| byte.is_ascii_digit())
            })
        })
}

pub fn diagnostics(cache: &Path) -> Result<Value> {
    let path = cache.join(report_contract::REPORT_PATH);
    let mut value: Value = read_json(&path)?;
    if !value.is_object() {
        return Err(invalid("LuaBuddy diagnostic report must be an object"));
    }
    let report: luabuddy_core::diagnostics::DiagnosticReport =
        serde_json::from_value(value.clone()).map_err(|e| {
            invalid(format!(
                "Invalid LuaBuddy diagnostic report {}: {e}",
                path.display()
            ))
        })?;
    if report.schema_version != report_contract::SCHEMA_VERSION
        || report.kind != report_contract::KIND
        || !valid_session(&report.session)
        || !valid_utc_timestamp(&report.started_at)
        || report
            .completed_at
            .as_ref()
            .is_some_and(|timestamp| !valid_utc_timestamp(timestamp))
        || !Operation::NAMES.contains(&report.operation.as_str())
    {
        return Err(invalid(
            "Unsupported or incomplete LuaBuddy diagnostic report identity",
        ));
    }
    if report
        .duration_ms
        .is_some_and(|duration| !duration.is_finite() || duration < 0.0)
    {
        return Err(invalid(
            "Diagnostic duration must be finite and nonnegative",
        ));
    }
    for key in ["loaded", "serverLoaded", "failed", "disabled", "pending"] {
        if !report.summary.contains_key(key) {
            return Err(invalid(format!("Diagnostic summary is missing {key}")));
        }
    }
    use luabuddy_core::diagnostics::ModStatus;
    let failed = report.error.is_some()
        || report.summary.get("failed").is_some_and(|count| *count > 0)
        || report
            .mods
            .values()
            .any(|detail| matches!(detail.status, ModStatus::Failed | ModStatus::Quarantined));
    let incomplete = report.completed_at.is_none()
        || report.duration_ms.is_none()
        || report
            .summary
            .get("pending")
            .is_some_and(|count| *count > 0)
        || report
            .mods
            .values()
            .any(|detail| detail.status == ModStatus::Prepared);
    value["ok"] = json!(!failed && !incomplete);
    value["reportStatus"] = json!(if failed {
        "failed"
    } else if incomplete {
        "incomplete"
    } else {
        "completed"
    });
    Ok(value)
}

pub fn doctor(game: &Path, cache: &Path) -> Result<Value> {
    let mut value = verify_install(game)?;
    let path = cache.join(report_contract::REPORT_PATH);
    if !path.exists() {
        value["lastInitialization"] = Value::Null;
        value["warnings"] = json!([
            "No runtime diagnostic report is available. Installed files do not establish successful in-game initialization."
        ]);
        return Ok(value);
    }
    match diagnostics(cache) {
        Ok(report) => {
            if report["ok"] == false {
                value["ok"] = json!(false);
            }
            if report["configDirectory"] != value["profiles"] {
                value["warnings"] = json!([
                    "The last runtime report used a different profile directory from the installed launcher generation."
                ]);
            }
            value["lastInitialization"] = report;
        }
        Err(error) => {
            value["ok"] = json!(false);
            value["lastInitializationError"] = json!(error.to_string());
        }
    }
    Ok(value)
}
