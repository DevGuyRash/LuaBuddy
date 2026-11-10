use serde_json::json;
use std::{
    env,
    fs::OpenOptions,
    io::Write,
    path::Path,
    process::Command,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

fn active(output: &[u8]) -> Result<bool, String> {
    match std::str::from_utf8(output).map(str::trim) {
        Ok("true") => Ok(true),
        Ok("false") => Ok(false),
        _ => Err("ScreenSaver GetActive did not return an unambiguous boolean".into()),
    }
}

fn query() -> Result<bool, String> {
    if env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none_or(|value| value.is_empty()) {
        return Err("Existing-display checks require an explicit DBUS_SESSION_BUS_ADDRESS".into());
    }
    let output = Command::new("/usr/bin/timeout")
        .args([
            "--signal=TERM",
            "--kill-after=1s",
            "2s",
            "/usr/bin/qdbus6",
            "org.freedesktop.ScreenSaver",
            "/ScreenSaver",
            "org.freedesktop.ScreenSaver.GetActive",
        ])
        .output()
        .map_err(|e| format!("ScreenSaver GetActive could not run: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "ScreenSaver GetActive unavailable or timed out: {}",
            output.status
        ));
    }
    active(&output.stdout)
}

pub(crate) fn require_unlocked(
    evidence: Option<&Path>,
    stage: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(path) = evidence else {
        return Ok(());
    };
    let started = Instant::now();
    let result = query();
    let allowed = matches!(result, Ok(false));
    let reason = match &result {
        Ok(false) => None,
        Ok(true) => {
            Some("The desktop is locked; refusing the existing-display operation".to_owned())
        }
        Err(reason) => Some(reason.clone()),
    };
    let record = json!({"schema":1,"unixMs":SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs_f64()*1000.0,"stage":stage,"active":result.as_ref().ok(),"allowed":allowed,"reason":reason,"queryMs":started.elapsed().as_secs_f64()*1000.0});
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    serde_json::to_writer(&mut file, &record)?;
    file.write_all(b"\n")?;
    match reason {
        Some(reason) => {
            Err(format!("Desktop session check failed before {stage}: {reason}").into())
        }
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::{active, require_unlocked};

    #[test]
    fn native_get_active_requires_one_boolean() {
        assert!(active(b"true\n").unwrap());
        assert!(!active(b" false\r\n").unwrap());
        for output in [
            b"".as_slice(),
            b"0",
            b"TRUE",
            b"false true",
            b"false\ntrue",
            b"false: denied",
            b"\xff",
        ] {
            assert!(active(output).is_err());
        }
    }

    #[test]
    fn owned_display_has_no_session_bus_requirement() {
        assert!(require_unlocked(None, "isolated native fixture").is_ok());
    }
}
