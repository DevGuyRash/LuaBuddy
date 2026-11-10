#![allow(dead_code)]

#[path = "../../../crates/luabuddy-host/src/lua.rs"]
mod lua;
#[path = "../../../crates/luabuddy-host/src/runtime_proof.rs"]
mod runtime_proof;

use jni::{InitArgsBuilder, JNIVersion, JavaVM};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let game = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("Provide the installed game directory")?
        .canonicalize()?;
    let agent = std::env::args_os()
        .nth(2)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/libluabuddy_host.so")
        })
        .canonicalize()?;
    let fixture = runtime_proof::Fixture::new()?;
    let agent_bytes = std::fs::read(&agent)?;
    let snapshot = fixture.root().join("libluabuddy-runtime-proof.so");
    std::fs::write(&snapshot, &agent_bytes)?;
    let options = InitArgsBuilder::new()
        .version(JNIVersion::V8)
        .option(format!(
            "-Djava.class.path={}",
            game.join("projectzomboid.jar").display()
        ))
        .option(format!(
            "-Djava.library.path={}",
            game.join("natives").display()
        ))
        .option(format!(
            "-Duser.home={}",
            fixture.root().join("home").display()
        ))
        .option(format!(
            "-Dluabuddy.config={}",
            fixture.root().join("config").display()
        ))
        .option(format!("-agentpath:{}", snapshot.display()))
        .option("-Dzomboid.steam=0")
        .option("-Xcheck:jni")
        .option("-Xverify:all")
        .option("--enable-native-access=ALL-UNNAMED")
        .build()?;
    std::env::set_current_dir(&game)?;
    println!(
        "Runtime proof artifact SHA256 {:x}",
        Sha256::digest(&agent_bytes)
    );
    let vm = JavaVM::new(options)?;
    let mut env = vm.attach_current_thread()?;
    if let Err(error) = runtime_proof::run(&mut env, &fixture) {
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_describe();
            let _ = env.exception_clear();
        }
        return Err(error.into());
    }
    println!("PASS production runtime parity proof");
    Ok(())
}
