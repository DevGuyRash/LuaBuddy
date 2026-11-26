//! Reads selected mods' files ahead of loading them.
//!
//! Loading reads hundreds of small Lua files while the game waits. On a slow or
//! busy disk that takes far longer than everything else together, so the files
//! are read in the background as soon as the game's Lua starts: once for the
//! menu, and again while a joined world is loading.

use crate::{game, log, lua::Context, selections};
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};

static RUNNING: AtomicBool = AtomicBool::new(false);

/// Failing to read ahead changes nothing but the time loading takes.
pub fn start(context: &mut Context<'_, '_>) {
    let found = selections::configured(context).and_then(|profiles| {
        let active = game::active(context)?;
        game::locate(context, &profiles, &active)
    });
    let mut roots = match found {
        Ok(found) => found
            .metadata
            .into_values()
            .flat_map(|metadata| metadata.roots)
            .collect::<Vec<_>>(),
        Err(error) => return log::debug("prefetch.skipped", error).write(),
    };
    roots.sort();
    roots.dedup();
    if roots.is_empty() || RUNNING.swap(true, Ordering::Relaxed) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("LuaBuddy prefetch".into())
        .spawn(move || {
            read_all(&roots);
            RUNNING.store(false, Ordering::Relaxed);
        });
    if let Err(error) = spawned {
        RUNNING.store(false, Ordering::Relaxed);
        log::debug("prefetch.skipped", error.to_string()).write();
    }
}

fn read_all(roots: &[PathBuf]) {
    let started = Instant::now();
    let (mut files, mut bytes) = (0_u64, 0_u64);
    for root in roots {
        read_tree(&root.join("media/lua"), 0, &mut files, &mut bytes);
    }
    log::debug(
        "prefetch.done",
        "Read the selected mods' files ahead of loading them",
    )
    .took(started.elapsed())
    .with(json!({ "roots": roots.len(), "files": files, "bytes": bytes }))
    .write();
}

fn read_tree(directory: &Path, depth: u32, files: &mut u64, bytes: &mut u64) {
    // Deeper than any real mod; a link cycle ends here.
    if depth > 16 {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut paths = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect::<Vec<_>>();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            read_tree(&path, depth + 1, files, bytes);
        } else if let Ok(content) = fs::read(&path) {
            *files += 1;
            *bytes += content.len() as u64;
        }
    }
}
