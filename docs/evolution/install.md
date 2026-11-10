# Install a native LuaBuddy package

Build the Rust workspace with the pinned toolchain and locked dependencies. An offline build needs those exact dependencies in the Cargo cache.

```sh
cargo build --release --locked --offline
```

Create a package using the native runtime and declarative profile catalog. The package command copies the current LuaBuddy executable, native runtime, license, and profile templates. It creates a versioned integrity manifest without copying game files or executable Lua hooks. Package entries beneath `profiles/` must be direct `.properties` files.

```sh
target/release/luabuddy package --runtime target/release/libluabuddy_host.so --profiles-dir profiles --out dist/LuaBuddy-linux-x86_64
target/release/luabuddy verify-package --package dist/LuaBuddy-linux-x86_64
```

Use a package from a source you trust. SHA256 checks detect changed package contents. They do not establish the author's identity or prove runtime compatibility.

Install into a discovered game, or supply its Steam installation directory. Installation probes the actual bundled JVM process architecture before changing launcher settings.

```sh
dist/LuaBuddy-linux-x86_64/luabuddy install --package dist/LuaBuddy-linux-x86_64 --game-dir /path/to/ProjectZomboid
```

A fresh installation selects no mods. Add an optional profile explicitly with `--mod`. Reinstalling preserves existing profiles, enabled settings, and user edits.

```sh
dist/LuaBuddy-linux-x86_64/luabuddy install --package dist/LuaBuddy-linux-x86_64 --game-dir /path/to/ProjectZomboid --mod AllInfo --mod AutoAll
```

Linux installation writes `ProjectZomboid64.site.json` beside the native launcher. It copies the existing site override when present, otherwise the base launcher configuration, and preserves unrelated JSON fields and JVM flags. Steam's base launcher JSON remains untouched. The override selects an immutable `.luabuddy/native/<package-manifest-sha256>/` generation through `-agentpath` and sets separate profile and template directory properties. Installing a profile template does not enable it in the selected configuration.

Fully restart Project Zomboid through its normal launcher after installation, update, repair, rollback, or uninstall. These operations do not control an already running game.

## Check installation files

Run the read-only inspection commands to verify the selected native generation, template integrity, launcher ownership, and configuration.

```sh
luabuddy verify --game-dir /path/to/ProjectZomboid
luabuddy doctor --game-dir /path/to/ProjectZomboid
luabuddy diagnostics --cache-dir /path/to/disposable-or-normal-game-cache
```

The commands return JSON. A failed inspection returns exit code 1. Invalid input or an operation failure returns exit code 2. Package and installation integrity results do not establish that an in-game workflow succeeded. Inspect the actual runtime diagnostics and the release verification report for that evidence.

## Use current native profiles

Profiles contain a native mod `id`, supported declarative fields, and optional compiled Rust `hook` identifiers. Validate the selected directory before installation.

```sh
luabuddy profiles --profiles-dir /path/to/profiles
```

The current profile format rejects unknown options, including every `plugin` assignment. The installer does not convert Lua hook files or classify obsolete Java-agent arguments. Remove obsolete LuaBuddy launcher entries and unsupported profile settings before installation. Unrelated JVM arguments remain preserved.

Use supported declarative behavior or an explicitly enabled [Rust extension](patcher.md) for custom integration logic. Native update, repair, rollback, transaction recovery, and uninstall remain available for current installations.

## Update, repair, or roll back

Use an integrity-checked local package for an update. The prior generation remains available for rollback.

Rollback also restores the prior generation's profile directory selection. If an update changed that selection from A to B, rollback selects A consistently in the launcher, installation metadata, verification output, and later updates without an explicit directory. Files in both profile directories remain untouched. Invalid saved directory arguments fail before the rollback transaction.

```sh
luabuddy update --package /path/to/new-package --game-dir /path/to/ProjectZomboid
luabuddy rollback --game-dir /path/to/ProjectZomboid
```

A modified launcher override prevents rollback rather than discarding user edits. Restore or merge those edits before retrying. A modified native runtime prevents ordinary replacement. Repair retains its bytes under `.luabuddy/recovery/` before restoring the package artifact.

```sh
luabuddy repair --package /path/to/original-package --game-dir /path/to/ProjectZomboid
```

A transaction journal records file contents before mutation. Launcher activation is the final installation write. The next mutating command recovers an interrupted transaction, and an explicit recovery command is available.

```sh
luabuddy recover --game-dir /path/to/ProjectZomboid
```

Recovery preserves a file that changed independently after interruption and names the file and journal that need attention. Concurrent installers use an owned process lock. Linux process identity includes the process start time so a reused process ID cannot make an old lock look current.

## Uninstall

Remove the owned integration and unchanged native generation files.

```sh
luabuddy uninstall --game-dir /path/to/ProjectZomboid
```

Uninstall preserves selected profiles, native repair recovery copies, Workshop content, and user-modified native files. It restores the original site override when safe. If the override has unrelated user changes, it removes only recorded LuaBuddy arguments and retains the changes. A repeated uninstall succeeds without additional changes.

Native launcher activation is currently evidenced on Linux. Windows and macOS runtime and installer behavior require native tests before a release can claim support. Workshop delivery alone does not activate this native runtime. Consult the platform capability report for the remaining one-click activation gate.
