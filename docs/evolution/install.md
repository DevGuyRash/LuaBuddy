# Install a native LuaBuddy package

Players can install without terminal commands through the [Workshop graphical setup guide](../../workshop/Contents/mods/LuaBuddy_SetupGuide/SETUP.md). Steam supplies `LuaBuddy-Setup-linux-x86_64.tar.gz` under the item's `mods/LuaBuddy_SetupGuide/common/setup` folder. Extract it with a desktop archive manager, keep the extracted package together, and double-click `luabuddy`. Graphical setup uses an existing Zenity or KDialog utility and the same Rust installer functions as the commands below. It finds the adjacent package independently of the file manager's working directory.

Close Project Zomboid before setup. Choose the game folder if discovery needs help, then choose **Install / update**. A fresh installation selects no mods. The setup interface also provides verification, repair, rollback, recovery, and removal. Cancel closes the current choice without requesting a mutation. Native runtime changes apply after the next normal Steam launch. Setup does not control a running game.

Steam updates the supplied archive. To activate a native update, extract the newly delivered package and open its setup interface again. Subscribing alone does not activate a JVM agent, and unsubscribing alone does not remove an installed native generation. The graphical flow targets native Linux x86_64; it does not establish Windows, macOS, Proton, or every Linux desktop environment as supported.

For Windows x64, use the [Windows CLI guide](../windows-cli.md), not the Linux Workshop archive. The Windows installer is experimental; game startup is a separate acceptance gate.

## Build and use the command line

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

Developer tooling can open graphical mode explicitly with `luabuddy setup --package /path/to/package --game-dir /path/to/ProjectZomboid`. Ordinary file-manager launches find the adjacent package without those arguments. Existing explicit CLI commands retain their JSON output.

Install into a discovered game, or supply its Steam installation directory. `luabuddy discover` lists every installation it found, the Steam libraries it searched, and library lists it could not read. `--package` defaults to the package the running executable is in. Installation probes the actual bundled JVM process architecture before changing launcher settings.

```sh
dist/LuaBuddy-linux-x86_64/luabuddy install --package dist/LuaBuddy-linux-x86_64 --game-dir /path/to/ProjectZomboid
```

A fresh installation selects no mods. Add an optional profile explicitly with `--mod`. Reinstalling preserves existing profiles, enabled settings, and user edits. The `mods` field of the result says whether each requested profile was `created`, was `alreadyEnabled`, or `existsDisabled`; a disabled profile stays disabled, and its entry gives the command that enables it.

```sh
dist/LuaBuddy-linux-x86_64/luabuddy install --package dist/LuaBuddy-linux-x86_64 --game-dir /path/to/ProjectZomboid --mod AllInfo --mod AutoAll
```

Linux installation writes `ProjectZomboid64.site.json` beside the native launcher. It copies the existing site override when present, otherwise the base launcher configuration, and preserves unrelated JSON fields and JVM flags. Steam's base launcher JSON remains untouched. Because the launcher reads the override instead of the base, the installation records the base it was built from. Install, update, repair, and rollback then carry your edits of the override over to Steam's current base: settings you did not change follow Steam, and settings you changed, such as `-Xmx`, stay. An override that existed before LuaBuddy is yours and does not follow Steam's base. The override selects an immutable `.luabuddy/native/<package-manifest-sha256>/` generation through `-agentpath` and sets separate profile and template directory properties. On a fresh install, the active configuration defaults to `.luabuddy/profiles` beside the game launcher. The `profiles` field from `luabuddy verify` reports the actual active directory, including a custom `--profiles-dir`. Commands that accept `--profiles-dir` use that directory when it is omitted. A directory reached through links, such as a linked `/home`, is recorded by its real path; directories LuaBuddy creates must not be links. Copying the template catalog does not enable any mods.

Fully restart Project Zomboid through its normal launcher after installation, update, repair, rollback, or uninstall. These operations do not control an already running game.

## Check installation files

Run the read-only inspection commands to verify the selected native generation, template integrity, launcher ownership, and configuration.

```sh
luabuddy verify --game-dir /path/to/ProjectZomboid
luabuddy doctor --game-dir /path/to/ProjectZomboid
luabuddy diagnostics --cache-dir /path/to/disposable-or-normal-game-cache
```

The commands return JSON. Verification fails when files are missing or modified, when the override does not select the installed generation, when Steam's base launcher changed since the override was built, or when an interrupted operation is recorded. Other differences in the override appear under `notes` and do not fail it. A failed inspection returns exit code 1. Invalid input or an operation failure returns exit code 2. Package and installation integrity results do not establish that an in-game workflow succeeded. Inspect the actual runtime diagnostics and the release verification report for that evidence.

## Use current native profiles

Profiles contain a native mod `id`, supported declarative fields, and optional compiled Rust `hook` identifiers. Validate the selected directory before installation.

```sh
luabuddy profiles --profiles-dir /path/to/profiles
```

The current profile format rejects unknown options, including every `plugin` assignment. The installer does not convert Lua hook files or classify obsolete Java-agent arguments. Remove obsolete LuaBuddy launcher entries and unsupported profile settings before installation. Unrelated JVM arguments remain preserved.

Use supported declarative behavior or an explicitly enabled [Rust extension](patcher.md) for custom integration logic. Native update, repair, rollback, transaction recovery, and uninstall remain available for current installations.

## Update, repair, or roll back

Use an integrity-checked local package for an update. The prior generation remains available for rollback, including after the same package is reinstalled or repaired. Older generations and the directories they leave empty are removed.

Rollback keeps the current override's other settings and restores the prior generation's own arguments. It also restores the prior generation's profile directory selection. If an update changed that selection from A to B, rollback selects A consistently in the launcher, installation metadata, verification output, and later updates without an explicit directory. Files in both profile directories remain untouched. Invalid saved directory arguments fail before the rollback transaction.

```sh
luabuddy update --package /path/to/new-package --game-dir /path/to/ProjectZomboid
luabuddy rollback --game-dir /path/to/ProjectZomboid
```

A modified file in the installed generation prevents reinstalling the same package. Repair retains its bytes under `.luabuddy/recovery/` before restoring the package artifact. An override that is not valid launcher JSON stops install, update, and rollback with a message that names it; repair and uninstall keep a copy under `.luabuddy/recovery/` and use the recorded settings. Repair accepts only the installed package and otherwise names both manifest digests; use update to change packages.

```sh
luabuddy repair --package /path/to/original-package --game-dir /path/to/ProjectZomboid
```

A transaction journal records the contents of the small files an operation changes before it changes them. Generation files are content-addressed and unused until the launcher selects them, so the journal records only their hashes. Launcher activation is the final installation write. Superseded files are removed only after the operation is committed. The next mutating command recovers an interrupted transaction, and an explicit recovery command is available.

```sh
luabuddy recover --game-dir /path/to/ProjectZomboid
```

Recovery removes the unmodified generation files an interrupted operation wrote. It preserves a file that changed independently after interruption and names the file and journal that need attention. `verify`, `doctor`, and setup name a recorded interruption even when no installation was completed. A journal that cannot be read is named, and uninstall moves it under `.luabuddy/recovery/` instead of failing. Concurrent installers are excluded by an operating-system lock on `.luabuddy/install.lock`. The system releases it when the installer exits, so the file can stay in place.

## Uninstall

Remove the owned integration and unchanged native generation files.

```sh
luabuddy uninstall --game-dir /path/to/ProjectZomboid
```

Uninstall preserves selected profiles, native repair recovery copies, Workshop content, and user-modified native files. It restores an override that existed before LuaBuddy, and removes an unedited one even if Steam's base changed meanwhile. If the override has your own changes, it removes only LuaBuddy's arguments, keeps the changes, and says so. A repeated uninstall succeeds without additional changes.

Native game launch is verified on Linux x86_64. The [experimental Windows x64 installer](../windows-cli.md) has installation, rollback, and stock-JDK tests, not verified gameplay. There is no macOS game-verified package. Graphical setup removes the terminal step from the accepted Workshop workflow, but it remains an explicit activation action. Consult the platform capability report for the remaining subscribe-only activation gate.
