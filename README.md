# LuaBuddy

LuaBuddy restores locally installed Project Zomboid client and shared Lua when a multiplayer connection resets the Lua environment to the server's mod list. Add a profile for a mod ID to load it. The loader has no built-in list of mod IDs. Bundled compatibility profiles and Lua hooks are optional; a fresh installation selects no mods.

LuaBuddy is a general-purpose **late client Lua loader**. It cannot make every kind of mod work on an arbitrary server. Mods that need server code, earlier startup events, maps, item definitions, Java patches, or assets registered during normal mod loading still need their normal installation. Auto All's worn-clothing repair synchronization requires its server component. A profile cannot supply that component.

The loader and native startup hook have been verified against Project Zomboid **42.21.0 Stable** and **42.22.0 Pre-Release** with **ZombieBuddy 2.3.4** and Java **25**. The 42.22 check uses official preview binaries; a full preview client/server play test remains unverified. See [compatibility](docs/compatibility.md) for the exact scope. Workshop source updates do not require rebuilding LuaBuddy.

## Install on Linux

Install Python 3.10 or newer and JDK 25 or newer. Install the mods through Steam Workshop or the game's local mods directory. Run these commands from this repository.

```sh
python3 luabuddy.py install
python3 luabuddy.py status
```

Installation downloads the current stable ZombieBuddy JAR when needed, builds LuaBuddy, and verifies the candidate through the real game classes and instrumented startup event before installing it. The JAR is cached locally and is not embedded in this repository. `build` prepares a verified local build without installing it.

The installer discovers Steam libraries. If discovery is ambiguous or the game lives elsewhere, pass `--game-dir` to each command. Both flat `projectzomboid.jar` and `java/projectzomboid.jar` layouts are supported, including a `projectzomboid` child directory.

```sh
python3 luabuddy.py install --game-dir /path/to/ProjectZomboid --mod AllInfo --mod AutoAll
```

## Install on Windows

Install Python 3.10 or newer and JDK 25 or newer, with `javac` and `jar` available through `JAVA_HOME` or `PATH`. Open PowerShell in this repository.

```powershell
py -3 luabuddy.py install
py -3 luabuddy.py status
```

Pass `--game-dir 'D:\SteamLibrary\steamapps\common\ProjectZomboid'` when the installer cannot find the game. Use the game's normal 64-bit launch mode. The alternative launch scripts do not read the launcher's JSON configuration.

## Start and update the game

After installation, quit the game normally and relaunch through Steam. Changes apply to the next launch. A running game keeps its current adapter until it exits.

The installer replaces the legacy `PZClientMods` adapter arguments when present and preserves unrelated launcher settings. It saves the previous launcher configuration before changing it. Running `install` again is safe. After rebuilding LuaBuddy or after a game update resets the launcher configuration, run `install` again.

If another ZombieBuddy agent or a combined patch list is already configured, installation stops before changing the launcher. Combine those patches manually rather than adding a second agent. Removal also leaves a combined agent untouched and reports that it needs manual editing.

Keep your `config` directory at the path printed by `status`. The launcher points to that directory. The installer stages runtime JARs beside the game so ZombieBuddy can use a relative patch path on Windows.

## Update ZombieBuddy

Check the installed JAR against the latest stable official release, then update when needed.

```sh
python3 luabuddy.py update --check
python3 luabuddy.py update
```

On Windows, use `py -3`. `update --check` fetches fresh release metadata without changing files. It returns `0` when current, `2` when an update is available, and `1` on error. The updater selects stable releases containing exactly `ZombieBuddy.jar`; GitHub's general latest release can instead be a Windows installer.

`build` and `install` check automatically when the verified local selection is missing or more than one hour old. Downloads must match GitHub's published SHA-256, asset size, and JAR manifest version. The candidate helper/framework pair must compile and pass the native startup-hook and game-API checks before promotion. A failed download, integrity check, or compatibility check leaves the installed files unchanged.

Verified JARs and release metadata are cached under `.local/dependencies/zombiebuddy/`. The build record ties the helper to its source inputs, game JARs, selected Java executable and runtime release metadata, and framework hash. An unchanged verified build is reused. Changes to that Java runtime trigger a new native compatibility check even when the game JAR is unchanged. For offline use, pass `--offline` to `build`, `install`, or `compatibility`. An automatic network failure can use a verified cached release with a warning. Explicit `update` fails if it cannot check the current release.

Checks happen when you run these commands. The Steam launch flow is unchanged, and a running JVM keeps its loaded agent until a full restart. The diagnostic report records the ZombieBuddy version loaded in that JVM when available. `doctor` shows installed and cached version information without contacting GitHub. `status` reports configured paths and file presence.

## Add another mod

Use the exact `id=` value from the mod's `mod.info`.

```sh
python3 luabuddy.py install --mod AnotherModID
```

For an unknown mod, the installer creates `config/AnotherModID.properties` containing `id=AnotherModID`. IDs with spaces, punctuation, Unicode, or Windows reserved names use a stable `mod-<hash>.properties` filename while preserving the mod ID inside. Quote such IDs on the command line, for example `--mod 'Spiffo Knew [B42]'`. For a bundled profile, the installer copies the profile and its Lua hook. Existing profiles stay intact. Restart the game after changing profiles. To disable one profile, set `enabled=false` in that file.

### Optional bundled profiles

Subscribe to the upstream Workshop items separately, then select only the profiles you want with `--mod`. Neither these mods nor their dependencies are selected automatically. For example, to add AllTheInfo and Auto All:

```sh
python3 luabuddy.py install --mod AllInfo --mod AutoAll
```

| Workshop mod | `--mod` ID | Integration |
| --- | --- | --- |
| [AllTheInfo](https://steamcommunity.com/sharedfiles/filedetails/?id=3781129962) | `AllInfo` | Existing tooltip, provider, and settings initialization. |
| [Auto All](https://steamcommunity.com/sharedfiles/filedetails/?id=3777237134) | `AutoAll` | Existing mechanics preloads and initialization check. |
| [Easy Pickings](https://steamcommunity.com/sharedfiles/filedetails/?id=3788245131) | `WalkLootForage` | Restore the saved forage key and loot priority. |
| [LootSense: Storage](https://steamcommunity.com/sharedfiles/filedetails/?id=3733326180) | `LootSenseStorage` | Restore its display settings. |
| [LootSense: QuickStack](https://steamcommunity.com/sharedfiles/filedetails/?id=3733862305) | `LootSenseQuickStack` | Restore settings; load after Storage when both are selected. Storage is optional. |
| [Craft Planner](https://steamcommunity.com/sharedfiles/filedetails/?id=3808092704) | `CraftPlanner` | Mount UI images, run its missed Neat Crafting initialization, restore settings and an already tracked window. |
| [NeatUI Inventory](https://steamcommunity.com/sharedfiles/filedetails/?id=3820447422) | `NeatUI_Inventory` | Requires `NeatUI_Framework`; mount images, rebuild existing inventory-page children through upstream code, and load before the bundled inventory/tooltip wrappers. |
| [Better Context Menu Icons](https://steamcommunity.com/sharedfiles/filedetails/?id=3830018449) | `SuperOl3gContextMenuIcons` | Mount images and register after Auto All and Alarm Syndrome when selected. |
| [What It Does](https://steamcommunity.com/sharedfiles/filedetails/?id=3823732502) | `WhatItDoes` | Uses upstream client Lua without a custom hook. |
| [Alarm Syndrome](https://steamcommunity.com/sharedfiles/filedetails/?id=3409527910) | `P4AlarmSyndrome` | Mount alarm icons, run its missed `OnLoad` initialization, and restore settings. |
| [ReloadAllAmmo](https://steamcommunity.com/sharedfiles/filedetails/?id=3774932027) | `ReloadAllAmmo` | Uses upstream client Lua and native optional Gunworks ordering without a custom hook. |
| [Auto Fishing](https://steamcommunity.com/sharedfiles/filedetails/?id=3804080377) | `AutoFishing` | Mount its UI image; use upstream fishing actions and sandbox-value fallbacks without a custom hook. |
| [Easy Drop'n'Loot](https://steamcommunity.com/sharedfiles/filedetails/?id=2871469707) | `KADropnLoot` | Restore its saved settings; wrap NeatUI Inventory when selected so upstream inventory buttons and category-transfer menus remain available. |
| [Area Tasks](https://steamcommunity.com/sharedfiles/filedetails/?id=3793163745) | `AreaTasks` | Uses upstream initialization, native tasks, and multiplayer sandbox-value fallbacks without a custom hook. |
| [NeatUI Framework](https://steamcommunity.com/sharedfiles/filedetails/?id=3508537032) | `NeatUI_Framework` | Optional dependency profile for NeatUI Inventory; mount the framework's UI images. |

For NeatUI Inventory, subscribe to both Workshop items and select both profiles unless the server already loads the framework:

```sh
python3 luabuddy.py install --mod NeatUI_Framework --mod NeatUI_Inventory
```

On Windows, use `py -3` in these examples. Existing selections remain when adding another profile. No new integration disables Auto All or AllTheInfo features. Other combinations and custom profiles continue to use native dependency metadata and explicit ordering. These profiles do not fix upstream gameplay bugs or provide missing server components; QuickStack keeps its upstream multiplayer transfer implementation.

Auto Fishing reads `SandboxVars.AutoFishing` when the world supplies it and otherwise uses its upstream defaults. Late client loading does not register its sandbox-options page or change the server's sandbox settings. Its custom seated bait-changing animation also requires normal animation-set loading; file mounting alone does not register that animation node. ReloadAllAmmo uses the game's ordinary magazine load/unload actions; Gunworks remains an optional upstream integration.

The installer rejects filename collisions before writing, including names that differ only by case on Windows. When using `--mod`, existing profiles must have one unindented `id=` entry with plain text, escaped spaces, or escaped backslashes. The runtime accepts Java properties syntax, but the installer refuses advanced or ambiguous identity syntax instead of guessing. Manage those profiles manually or use the conventional `id=` form.

The game's own mod discovery finds installed mods and chooses their `common` and version directories. To select a mod outside those directories, add `path=/absolute/path/to/mod` to its profile. Point at the directory above `42`, `42.20`, and `common`, so version selection can continue to follow upstream updates. Use forward slashes in Windows properties paths, such as `path=D:/Mods/Example`. Relative paths resolve against the profile's directory.

## Profile reference

Profiles are UTF-8 Java properties files in `config`. Each file has one mod ID. Lists use semicolons. Duplicate IDs, unknown options, missing required mods, and cyclic load order produce diagnostics in the game log. Install each required dependency as a profile too, unless the server already enables it.

| Option | Meaning |
| --- | --- |
| `id` | Required mod ID matching `mod.info`. |
| `enabled` | `true` by default. Set `false` to skip this profile. |
| `path` | Optional mod root. Otherwise use native installed-mod discovery. |
| `workshop` | Optional numeric Workshop item ID for a mod packaged directly at the item root. Native mod-ID discovery takes precedence; otherwise Steam locates the item and the native parser reads its root `mod.info`. Set `path` as well when Steam is unavailable. |
| `requires` | Additional required mod IDs. Native `require=` dependencies always apply. |
| `after` | Optional ordering after other configured IDs. Native `loadAfter` also applies. |
| `before` | Optional ordering before other configured IDs. Native `loadBefore` also applies. |
| `exclude` | Glob patterns relative to `media/lua`, such as `client/Example/SelfTest.lua`. |
| `excludeWhen.ModID` | Additional exclusions when that mod is active on the server. |
| `preload` | Modules to require before this mod's shared and client files execute. |
| `plugin` | Optional Lua hook file relative to this profile's directory. |
| `mount` | `false` by default. Set `true` to publish the mod's common/version file paths through the game's native `loadMod` API, including loose UI images. Does not activate the mod or execute its server Lua. |
| `replay` | Optional `OnGameBoot` and/or `OnLoad`. Run only callbacks registered by this mod's restored Lua for those missed events, in the listed event order, after its client files and before its `after` hook. Callbacks receive no arguments. |

For example, a mod with no special compatibility rules needs only its ID. Add the other options only when the mod requires them.

```properties
id=Example
exclude=client/Example/SelfTest.lua
preload=Vehicles/ISUI/ISVehicleMechanics
plugin=Example.lua
```

A plugin returns a table with optional `before(context)` and `after(context)` functions. Keep imports and initialization inside those hooks. `before` runs before the shared phase. `after` runs after that mod's client phase, before dependent client files. `context.id` is the mod ID, `context.active` contains server-loaded and successfully loaded local IDs, and `context.state.mods` holds each profile's status. Plugins run trusted local code in the game's Lua environment.

```lua
return {
    after = function(context)
        print("Initialized " .. context.id)
    end,
}
```

## Loading behavior

ZombieBuddy applies one hook immediately before the normal `OnGameStart` event. LuaBuddy runs only on multiplayer clients. Single player and dedicated servers use their normal loading paths. A mod already present in the server's active list is skipped. Each Lua environment initializes once, including after disconnecting and rejoining in the same JVM.

LuaBuddy reads upstream files without editing them. Version-specific files replace matching common files. It compiles a mod's sources before executing them, loads the shared phase before the client phase, and orders mods using native dependency metadata. Files with the same relative path follow that mod order, with later mods taking precedence. Shared and client files with the same module name remain distinct scripts. `require` prefers shared modules and caches the first return value, including `nil` and `false`.

An environment-local `require` resolver remains available for imports from later callbacks and other configured mods. Imports outside the local registry go to the game's original `require`. LuaBuddy uses the game's translation parser for language fallback and placeholders. It does not change the server mod list or the checksum calculation.

Failures appear in `console.txt` under `[LuaBuddy]`, and in `LuaBuddy.mods` inside Lua. A failed mod is not retried in the same environment because Lua side effects cannot be rolled back. Unrelated mods can continue. A dependent mod fails when its required mod fails. Fix the cause and reconnect or restart.

Late loading does not replay the game's startup lifecycle. A profile can opt into `replay` for its own missed `OnGameBoot` or `OnLoad` callbacks when they are safe to run late. Other mods' callbacks are not triggered. Native event subscriptions remain intact; use replay only for initialization that is safe if the game later emits the event again. Replay does not reproduce event arguments, earlier game state, or callbacks registered outside that mod's restored sources.

Opt-in file mounting exposes the upstream mod's files through the native virtual filesystem, with normal common/version precedence and later mounted mods winning path collisions. Images remain subject to the game's path validation: use native-discovered mod roots for assets. An explicit external `path` can supply Lua without making that path valid for native image loading. Mounting does not load texture packs, animation sets, item definitions, maps, or server code, invalidate previously cached textures, or change the server's active mod list. Files mounted or callbacks registered before a failure can remain in that environment. Reconnect or restart after fixing a failure. Calls that require full mod activation and mods that replace `require` may still need additional integration. LuaBuddy does not automatically turn client code into server code.

Easy Pickings currently packages `mod.info` and `media` directly at its Workshop item root. Its profile's `workshop` fallback handles that layout without moving or copying its files. Native versioned discovery takes over if upstream adopts the normal mod layout. With Steam disabled, set this profile's `path` to the installed item's root.

## Diagnose loading problems

Run the read-only installation check and inspect the last loader initialization report.

```sh
python3 luabuddy.py doctor
python3 luabuddy.py diagnostics
python3 luabuddy.py diagnostics --json
```

On Windows, use `py -3` instead of `python3`. `doctor` checks launcher arguments, the configured debug value, runtime files, profiles, and the last initialization report. A missing report produces a warning. It does not mean the current game has loaded the mods successfully. The JSON forms support scripts and return a nonzero exit code for diagnosed failures or invalid data.

Each multiplayer Lua environment writes one `LuaBuddy/diagnostics.json` file beneath the game's cache directory. The default is `~/Zomboid/LuaBuddy/diagnostics.json` on Linux and `%USERPROFILE%\Zomboid\LuaBuddy\diagnostics.json` on Windows. If you launch the game with a custom `-cachedir`, pass that directory to either diagnostic command.

```sh
python3 luabuddy.py doctor --cache-dir /path/to/game-cache
python3 luabuddy.py diagnostics --cache-dir /path/to/game-cache --json
```

The report records the session ID, UTC timestamps, loader/game/Java versions, selected profile and source paths, loaded/skipped/disabled/failed counts, initialization stages, and available Lua/Java error traces. It describes the last attempted loader initialization. Later gameplay callbacks continue through the game's normal error logging. An old successful report can remain if a later launch never reaches LuaBuddy, so check its session and completion time.

Normal logging writes a startup summary, per-mod result, and report location to the game's existing console logs under `[LuaBuddy]`. To record each initialization stage and source filename, enable debug logging and restart the game.

```sh
python3 luabuddy.py install --debug
```

Use `install --no-debug` to turn detailed logging off. Ordinary reinstallations preserve the chosen setting. Report-write failures appear as warnings and do not prevent healthy mods from loading. Reports stay local and contain selected file paths and unredacted error text. The collector does not read complete game logs, unrelated launcher settings, environment variables, or player/server/save records.

## Remove LuaBuddy

```sh
python3 luabuddy.py remove
```

On Windows, use `py -3 luabuddy.py remove`. Restart the game afterward. Removal preserves unrelated launcher settings and your profiles.

## Build and verify

`luabuddy.py build` downloads or reuses a verified stable ZombieBuddy JAR, compiles against the selected game's classes, and runs the native compatibility gate. It emits Java 25 bytecode in `dist/LuaBuddy.jar` and provenance in `dist/LuaBuddy.build.json`. There is no Maven, Gradle, Python package installation, or replacement Java patch framework.

To verify another game installation without installing the helper or replacing the current build, run:

```sh
python3 luabuddy.py compatibility --game-dir /path/to/preview --offline
```

The compatibility check uses temporary files inside that game directory and writes its last log to `.local/compatibility.log`. It verifies native APIs and the actual instrumented `OnGameStart` event. It does not join a multiplayer server or establish compatibility with every server anti-cheat or mod combination.

Installation prepares every changed file before replacing runtime files and activates the launcher last. Ordinary replacement failures attempt to restore the previous files. The original launcher backup is published atomically. This protects failed writes; it is not a transaction across a power loss or filesystem failure.

See [verification](docs/verification.md) for checks, evidence, and runtime limits. See [third-party software](THIRD_PARTY.md) for upstream references and licenses.
