#![allow(dead_code)]

#[path = "../../../crates/luabuddy-host/src/steam.rs"]
mod steam;

use jni::{InitArgsBuilder, JNIVersion, JavaVM};
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let game = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("Provide the installed game's root directory")?,
    );
    let path = game.join("natives/libsteam_api.so");
    let java = game.join("projectzomboid.jar");
    if !path.is_file() || !java.is_file() {
        return Err("Expected the installed Linux game JAR and Steam library".into());
    }
    // SAFETY: This isolated proof deliberately opens the user's unmodified host
    // library for symbol resolution. It calls no Steam initialization or account API.
    let library = unsafe { libloading::Library::new(&path)? };
    for name in [
        "SteamAPI_SteamUGC_v021",
        "SteamAPI_SteamUtils_v010",
        "SteamAPI_GetHSteamUser",
        "SteamAPI_ISteamUtils_GetAPICallResult",
        "SteamAPI_ISteamUtils_IsAPICallCompleted",
        "SteamAPI_ISteamUGC_CreateQueryAllUGCRequestPage",
        "SteamAPI_ISteamUGC_CreateQueryUGCDetailsRequest",
        "SteamAPI_ISteamUGC_SetSearchText",
        "SteamAPI_ISteamUGC_SetReturnLongDescription",
        "SteamAPI_ISteamUGC_SetReturnChildren",
        "SteamAPI_ISteamUGC_SendQueryUGCRequest",
        "SteamAPI_ISteamUGC_GetQueryUGCResult",
        "SteamAPI_ISteamUGC_GetQueryUGCChildren",
        "SteamAPI_ISteamUGC_ReleaseQueryUGCRequest",
        "SteamAPI_ISteamUGC_GetItemState",
        "SteamAPI_ISteamUGC_GetItemInstallInfo",
        "SteamAPI_ISteamUGC_GetItemDownloadInfo",
        "SteamAPI_ISteamUGC_GetNumSubscribedItems",
        "SteamAPI_ISteamUGC_GetSubscribedItems",
        "SteamAPI_ISteamUGC_SubscribeItem",
        "SteamAPI_ISteamUGC_UnsubscribeItem",
        "SteamAPI_ISteamUGC_DownloadItem",
    ] {
        let name_bytes = std::ffi::CString::new(name)?;
        // SAFETY: Resolve an address only; no guessed function is executed.
        unsafe { library.get::<*mut std::ffi::c_void>(name_bytes.as_bytes_with_nul())? };
    }
    println!("PASS: all 22 current-host Workshop symbols exist; no Steam API was called");
    let args = InitArgsBuilder::new()
        .version(JNIVersion::V8)
        .option(format!("-Djava.class.path={}", java.display()))
        .option("-Dzomboid.steam=0")
        .build()?;
    let vm = JavaVM::new(args)?;
    let mut env = vm.attach_current_thread()?;
    match steam::SteamWorkshop::attach_existing(&mut env, &path) {
        Err(error) if error.0.contains("without Steam") => {
            println!(
                "PASS: actual host SteamUtils rejects Workshop access before Steam initialization"
            );
        }
        Err(error) => return Err(format!("Unexpected unavailable-context failure: {error}").into()),
        Ok(_) => return Err("Unexpected Workshop context in an uninitialized proof JVM".into()),
    }
    println!(
        "NOT RUN: live search, subscribe, download, and in-game UI need an owned initialized game context"
    );
    Ok(())
}
