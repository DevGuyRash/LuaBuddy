# LuaBuddy

LuaBuddy is an in-game mod manager for Project Zomboid Build 42. It can load eligible client/shared Lua into a multiplayer client, manage mods for the next normal startup, and show which mods actually loaded. Its loader and JVM patch engine are written in Rust. It runs upstream Lua using the game's own JVM and Kahlua interpreter; players do not need another loader, interpreter, or compiler.

A fresh install enables **no mods**. Install the upstream mods you want separately. A Workshop subscription delivers files; it does not activate LuaBuddy or select any of its optional mod profiles.

## Which download do I need?

| Platform | Where to get it | What has been verified |
| --- | --- | --- |
| Native Linux x86_64 (glibc 2.34+) | [Steam Workshop](https://steamcommunity.com/sharedfiles/filedetails/?id=3838682402) or the [v0.1.0-rc.3 GitHub prerelease](https://github.com/DevGuyRash/LuaBuddy/releases/tag/v0.1.0-rc.3) | Native game launch and in-game flows on Build 42.22.2. [Compatibility and test scope](docs/compatibility.md). |
| Native Windows x64 | [Experimental command-line package from GitHub Actions](docs/windows-cli.md), not the Workshop archive or rc.3 release | Installation, rollback, and DLL startup in a test directory with a stock JDK, **not** a licensed game launch. Use Steam itself for Workshop delivery. |
| macOS or Proton | No supported game package | No verified game execution. |

On Linux, close the game, extract the whole archive and double-click `luabuddy` in the extracted folder. Choose **Install / update**, then start the game normally through Steam. [Graphical setup and removal](workshop/Contents/mods/LuaBuddy_SetupGuide/SETUP.md) use an existing Zenity or KDialog utility; without one, use the command line below. To apply a native update, repeat setup from the new archive. Unsubscribing alone does not uninstall the runtime.

## Build and install

These commands build a Linux source checkout. If you already extracted a Linux package, open a terminal in its folder and run `./luabuddy verify-package --package .`, then `./luabuddy install --package .`. See the [installer reference](docs/evolution/install.md) for custom locations and maintenance.

Build with the pinned Rust toolchain and lockfile, then create an integrity-checked package. Do not copy proprietary game files into a package.

```sh
cargo build --release --locked
target/release/luabuddy package --runtime target/release/libluabuddy_host.so --profiles-dir profiles --out dist/LuaBuddy-linux-x86_64
target/release/luabuddy verify-package --package dist/LuaBuddy-linux-x86_64
dist/LuaBuddy-linux-x86_64/luabuddy install --package dist/LuaBuddy-linux-x86_64 --game-dir /path/to/ProjectZomboid
```

Close the game before setup, then relaunch through Steam. Installation writes a reversible `ProjectZomboid64.site.json` override and leaves Steam's base launcher JSON intact. Setup does not stop or restart a running game. A running JVM keeps its current library until exit. See [installation, repair, rollback, and uninstall](docs/evolution/install.md).

The installer keeps the package's profile templates separate from your active configuration. Reinstalling does not overwrite personal profiles. To select specific integrations from an extracted package:

```sh
./luabuddy install --package . --mod AllInfo --mod AutoAll
```

Use `--game-dir` or `PZ_GAME_DIR` when Steam discovery does not identify one installation. `luabuddy discover`, `verify`, `doctor`, and `diagnostics` return JSON. File verification establishes installation integrity; the runtime report and actual game checks establish whether loading succeeded.

## Manage mods in the game

Open **Options > Mods > LuaBuddy > Manage mods**. Browse installed content or search Workshop through the initialized game Steam context. Details show identity, available metadata, dependencies, source, and observed delivery and activation states. Network failures and unavailable items remain visible as failures.

Related actions are grouped under **This session**, **Next launch**, **Workshop**, and **Diagnostics**. Action feedback distinguishes saved selections, actual runtime loading, pending delivery, and failures. **Help & guides** covers complete workflows. See [common tasks](docs/workflows.md) for local testing, one-launch selections, downloads, reload, and troubleshooting.

An explicit Add or Download action requests the corresponding Steam operation. A subscribed item remains distinct from an installed item, and installed content remains distinct from a loaded profile. Closing a download view stops observation where native cancellation is unavailable; Steam can continue downloading.

For eligible multiplayer client/shared Lua, choose **This session > Enable late**, then **Load enabled**. **Load new profiles** on the LuaBuddy Options page also refreshes selections without duplicating loaded modules or callbacks. For the game's normal startup path, use **Next launch > Startup enable** and restart. The two selections are independent; apply or discard pending Options edits before changing either. [Runtime behavior](docs/runtime.md) defines native-startup, missing-server-code, ordering, and lifecycle limits.

**Reload a profile** requires explicit upstream cleanup cooperation. LuaBuddy compiles and validates the replacement before calling the running generation's captured cleanup function. A refusal retains the old generation. Uncertain cleanup or failed activation quarantines it and requires reconnecting. Bundled profiles do not gain safe reload simply because they load successfully.

Local mods use the game's native discovery or an explicit `path=` root. The CLI can save a generic local selection with `select --profiles-dir DIR --mod ID --path ROOT`. Add `--once` to queue it for one game process, including reconnects. LuaBuddy consumes only its temporary registration and leaves source directories and normal profiles intact. `deselect` disables a future selection, while `deselect --once` cancels an unclaimed registration. See [the full testing workflow](docs/workflows.md#test-a-mod-for-one-game-launch).

## Select optional integrations

The table lists the templates in this source checkout. The published **v0.1.0-rc.3 Linux archive** contains 24 templates; **Auto Share Map Markers**, **PourIntoContainer**, and **Shelf Aware!** were added on `main` after that release. Install from a source build containing those templates to use their bundled integrations. Check an extracted package's `profiles/` directory to see what it includes. Unlisted mods can still be selected by native ID, but an ID alone does not apply a template's exclusions or other compatibility settings.

Install the upstream content and choose only the integrations you want. Native dependency metadata remains authoritative. Select additional dependency profiles only when the server or native loader does not already supply them.

| Workshop mod | `--mod` ID | Integration |
| --- | --- | --- |
| [AllTheInfo](https://steamcommunity.com/sharedfiles/filedetails/?id=3781129962) | `AllInfo` | Restore settings and runtime moodles, retain its opt-in SelfTest, and attach its custom Options tab when added during play. |
| [Auto All](https://steamcommunity.com/sharedfiles/filedetails/?id=3777237134) | `AutoAll` | Native mechanics preloads, initialization check, and scoped runtime startup initialization. |
| [Easy Pickings](https://steamcommunity.com/sharedfiles/filedetails/?id=3788245131) | `WalkLootForage` | Restore the saved forage key and loot priority. |
| [LootSense: Storage](https://steamcommunity.com/sharedfiles/filedetails/?id=3733326180) | `LootSenseStorage` | Restore its display settings. |
| [LootSense: QuickStack](https://steamcommunity.com/sharedfiles/filedetails/?id=3733862305) | `LootSenseQuickStack` | Restore settings; discover optional Storage dynamically, including when it is added later. |
| [Craft Planner](https://steamcommunity.com/sharedfiles/filedetails/?id=3808092704) | `CraftPlanner` | Mount UI images, run its missed Neat Crafting initialization, restore settings and an already tracked window. |
| [NeatUI Inventory](https://steamcommunity.com/sharedfiles/filedetails/?id=3820447422) | `NeatUI_Inventory` | Requires `NeatUI_Framework`; mount images, rebuild existing pages through upstream code, restore native/NeatUI layouts, preserve current runtime geometry and pane preferences, and load before the bundled inventory/tooltip wrappers. |
| [Better Context Menu Icons](https://steamcommunity.com/sharedfiles/filedetails/?id=3830018449) | `SuperOl3gContextMenuIcons` | Mount images and register after Auto All and Alarm Syndrome when selected. |
| [What It Does](https://steamcommunity.com/sharedfiles/filedetails/?id=3823732502) | `WhatItDoes` | Uses upstream client Lua without a custom hook. |
| [Alarm Syndrome](https://steamcommunity.com/sharedfiles/filedetails/?id=3409527910) | `P4AlarmSyndrome` | Mount alarm icons, run its missed `OnLoad` initialization, and restore settings. |
| [ReloadAllAmmo](https://steamcommunity.com/sharedfiles/filedetails/?id=3774932027) | `ReloadAllAmmo` | Mount radial icons; use upstream client Lua and native optional Gunworks ordering without a custom hook. |
| [Auto Fishing](https://steamcommunity.com/sharedfiles/filedetails/?id=3804080377) | `AutoFishing` | Mount its UI image; use upstream fishing actions and sandbox-value fallbacks without a custom hook. |
| [Easy Drop'n'Loot](https://steamcommunity.com/sharedfiles/filedetails/?id=2871469707) | `KADropnLoot` | Restore its saved settings; wrap NeatUI Inventory when selected so upstream inventory buttons and category-transfer menus remain available. |
| [Area Tasks](https://steamcommunity.com/sharedfiles/filedetails/?id=3793163745) | `AreaTasks` | Uses upstream initialization, native tasks, and multiplayer sandbox-value fallbacks without a custom hook. |
| [Loot Tracker](https://steamcommunity.com/sharedfiles/filedetails/?id=3823828778) | `LootTrackerB42` | Mount map icons, restore saved preferences, and use upstream map tracking, local multiplayer saves, and the vanilla respawn-wave countdown. |
| [Auto Share Map Markers](https://steamcommunity.com/sharedfiles/filedetails/?id=3796750635) | `AutoShareMapMarkers` | New multiplayer map symbols and notes default to **Everyone**. The upstream mod remembers your last visibility choice. |
| [PourIntoContainer](https://steamcommunity.com/sharedfiles/filedetails/?id=3735378533) | `PourIntoContainer` | Restores the upstream inventory menu but omits its options module so vanilla liquid controls stay available in a fresh session. Its custom Pour/Empty actions are not supplied to a server; use **Fluid > Transfer Liquid** or **Fluid > Empty** on a server without them. |
| [Shelf Aware!](https://steamcommunity.com/sharedfiles/filedetails/?id=3775396697) | `PinyosShelfAware` | Manual reference window: book/recipe/media catalogue, read progress, ownership marks and memos. Excludes automatic location scans, loot badges and server backup. Manual notes are not a durable local save. |
| [True Line of Sight](https://steamcommunity.com/sharedfiles/filedetails/?id=3792740760) | `TrueLineOfSight` | Mount its fallback outline image and restore saved preferences; use upstream player and render callbacks without startup replay. |
| [Batch Craft Recipe](https://steamcommunity.com/sharedfiles/filedetails/?id=3584890848) | `vac_mod_b42_10` | Use upstream right-click batch modes, native crafting actions, and craft-recipe tag changes without a custom hook. |
| [Nutrition Helper UI](https://steamcommunity.com/sharedfiles/filedetails/?id=3803702351) | `NutritionHelper` | Restore saved preferences and attach the Health-tab button; load before AllTheInfo when both are selected so both food-tooltip renderers remain available. |
| [Quick Build Menu](https://steamcommunity.com/sharedfiles/filedetails/?id=3741839491) | `QuickBuildMenu` | Preload the native build action, restore saved options, and preserve handcraft favorites during upstream startup cleanup. |
| [NeatUI Mechanics](https://steamcommunity.com/sharedfiles/filedetails/?id=3832114824) | `NeatUI_Mechanics` | Requires `NeatUI_Framework`; mount images, restore preferences, and use upstream setup for existing mechanics windows and sidebars while preserving current window dimensions. |
| [Better Crafting](https://steamcommunity.com/sharedfiles/filedetails/?id=3618123324) | `EURY_CRAFTING` | Restore and apply saved preferences; use upstream recipe browsing, XP tooltips, sorting, ingredient memory, and favorites. |
| [Banger's Retro Car Dashboard](https://steamcommunity.com/sharedfiles/filedetails/?id=3739256322) | `RetroDashboard` | Mount its images and use the upstream dashboard factory for existing player UI when loading during play. |
| [Mod Manager Reforged (Client)](https://steamcommunity.com/sharedfiles/filedetails/?id=3737874810) | `MMReforgedC` | Mount images and run its missed boot patch registration. Enable it normally in the game's Mods menu for the main-menu manager. |
| [NeatUI Framework](https://steamcommunity.com/sharedfiles/filedetails/?id=3508537032) | `NeatUI_Framework` | Optional dependency profile for NeatUI Inventory and Mechanics; mount the framework's UI images. |

For PourIntoContainer and Shelf Aware!, [select the complete templates](docs/workflows.md#pour-and-shelf-aware-on-a-multiplayer-client), not an ID-only profile.

Existing profiles and settings survive adding another selection. Auto All and AllTheInfo remain independent optional integrations. The integration code lives in `profiles/hooks.rs`; generic loader and patcher code does not select a default mod set.

Late client loading cannot register every native asset, execute missing server code, or reconstruct earlier world state. Auto Fishing's custom animation and sandbox-page setup require normal loading. Alarm Syndrome's server-synchronized custom action needs its server component. Better Crafting's server fixes are not supplied by its client profile. Mod Manager Reforged's own initial-menu integration requires normal native activation. LuaBuddy's manager edits LuaBuddy selections rather than replacing that mod's presets.

Declared order also applies to already running mods. Nutrition Helper must precede AllTheInfo. NeatUI Inventory must precede the wrappers named by its profile. An incompatible runtime addition is refused before mutation. Use a compatible new connection or normal native loading.

Upstream gameplay limitations remain upstream. File mounting does not register maps, item definitions, animation sets, or every texture cache. The profiles preserve the game's active server list and do not bypass checksums or server authority.

## Configure a profile

Use the exact native `id=` from the mod's `mod.info`. An arbitrary mod starts with a declarative ID-only profile.

```properties
id=Example
```

Profiles support native-root discovery or an explicit `path`, dependency and optional ordering metadata, source exclusions, preloads, scoped replay, opt-in mounting, explicit live-loading policy, Rust hook identifiers, and a bounded upstream cleanup path. See the [profile and lifecycle reference](docs/runtime.md).

Custom first-party integration logic belongs in a compiled Rust hook or an explicitly enabled Rust extension. The installer does not translate old Lua hook files or import their configuration.

## Develop patches and extensions

The reusable Rust registry provides exact targets and fingerprints, ownership, deterministic priority and dependencies, explicit capability negotiation, receipt-based installation, conflict diagnostics, and scoped disable. The JVM backend currently supports entry observation, integer argument changes, and integer-like replacement. It rejects unsupported after, around, reference-replacement, and result-edit operations.

Compiled Rust extensions use a versioned C ABI. Loading requires exact artifact, provenance, privilege, install, and enable approval. A downloaded Workshop library has no automatic native execution permission. Native extensions have the process's OS privileges. Read the [Rust patcher API and ABI](docs/evolution/patcher.md) and [JVM backend contract](docs/evolution/jvm-patcher.md).

## Verify a checkout

Run the portable aggregate without proprietary game files, or supply a supported installed game for native proofs.

```sh
cargo xtask check --portable --offline
cargo xtask check --offline --game-dir /path/to/ProjectZomboid
cargo xtask check --offline --audit --game-dir /path/to/ProjectZomboid
```

An offline build needs the locked dependencies and advisory database already cached. [Verification](docs/verification.md) describes fetching those development inputs, evidence, exact proof scope, and separate graphical and performance gates. The [migration verification report](docs/evolution/verification-report.md) and [behavior ledger](docs/evolution/behavior.md) record earlier artifact-specific results; the [original migration request](docs/evolution/request.md) describes goals rather than shipped features. The [rc.3 event-dispatch benchmark](docs/evolution/event-dispatch.md) covers a headless workload, not game FPS. [Publishing](docs/publishing.md) covers the release and Workshop listing.
