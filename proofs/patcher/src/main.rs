use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;
use std::time::{Duration, Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 5 {
        return Err("Supply bundled java, game jar, proof agent library, math extension and observer extension paths".into());
    }
    let java = PathBuf::from(&args[0]).canonicalize()?;
    let jar = PathBuf::from(&args[1]).canonicalize()?;
    let agent = PathBuf::from(&args[2]).canonicalize()?;
    let math = PathBuf::from(&args[3]).canonicalize()?;
    let observer = PathBuf::from(&args[4]).canonicalize()?;
    println!(
        "Host game JAR SHA256 {}",
        luabuddy_patcher::bytecode_sha256(&std::fs::read(&jar)?)
    );
    let mut child = Command::new(java)
        .arg("-Xcheck:jni")
        .arg("-Xverify:all")
        .arg(format!("-agentpath:{}", agent.display()))
        .arg(format!("-Dluabuddy.proof.math={}", math.display()))
        .arg(format!("-Dluabuddy.proof.observer={}", observer.display()))
        .arg("-cp")
        .arg(jar)
        .arg("-version")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let started = Instant::now();
    while child.try_wait()?.is_none() {
        if started.elapsed() > Duration::from_secs(30) {
            child.kill()?;
            let _ = child.wait();
            return Err("The isolated JVM patcher proof exceeded its 30-second deadline".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output()?;
    let transcript = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    print!("{transcript}");
    if !output.status.success()
        || !transcript.contains("[LuaBuddy patcher proof] PASS live JVM patches")
        || transcript.contains("FAIL")
        || transcript.contains("WARNING in native method")
        || transcript.contains("FATAL ERROR")
    {
        return Err("The actual JVM patcher proof did not pass cleanly".into());
    }
    Ok(())
}
