# LuaBuddy

LuaBuddy is a Rust-native client/shared Lua loader, in-game mod manager, and reusable JVM patcher for Project Zomboid. The native runtime uses the game's existing Java classes and Kahlua interpreter. It has no ZombieBuddy, Python, extra interpreter, or runtime compiler dependency.

A fresh installation selects no mods. Bundled profiles are optional integrations. Profiles use supported Rust hooks and declarative fields. Obsolete `plugin` settings and Java-agent or Lua-hook migration are unsupported. Workshop subscription, download, installation, native activation, profile selection, and live loading are separate states.

The native adapter and installer currently target Linux x86_64. Windows and macOS game execution remain unverified. Steam Workshop delivery does not yet provide a demonstrated subscribe-only activation route for this native runtime. [Compatibility](docs/compatibility.md) records that gate without treating an installer as one-click Workshop setup.

This Rust migration is a release candidate. The [verification report](docs/evolution/verification-report.md) records the exercised functional paths and the open performance and Workshop acceptance gates.

## Build and install

Build with the pinned Rust toolchain and lockfile, then create an integrity-checked package. Do not copy proprietary game files into a package.

```sh
cargo build --release --locked
target/release/luabuddy package --runtime target/release/libluabuddy_host.so --profiles-dir profiles --out dist/LuaBuddy-linux-x86_64
target/release/luabuddy verify-package --package dist/LuaBuddy-linux-x86_64
dist/LuaBuddy-linux-x86_64/luabuddy install --package dist/LuaBuddy-linux-x86_64 --game-dir /path/to/ProjectZomboid
```

Quit and relaunch the game through its normal native launcher. Installation writes a reversible `ProjectZomboid64.site.json` override and leaves Steam's base launcher JSON intact. A running JVM keeps its current library until exit. See [installation, repair, rollback, and uninstall](docs/evolution/install.md).

The installer copies the full declarative template catalog into an immutable generation. It copies only explicitly selected profiles into the active configuration.

```sh
luabuddy install --package dist/LuaBuddy-linux-x86_64 --mod AllInfo --mod AutoAll
```

Use `--game-dir` or `PZ_GAME_DIR` when Steam discovery does not identify one installation. `luabuddy discover`, `verify`, `doctor`, and `diagnostics` return JSON. File verification establishes installation integrity; the runtime report and actual game checks establish whether loading succeeded.

## Manage mods in the game

Open **Options > Mods > LuaBuddy > Manage mods**. Browse installed content or search Workshop through the initialized game Steam context. Details show identity, available metadata, dependencies, source, and observed delivery and activation states. Network failures and unavailable items remain visible as failures.

An explicit Add or Download action requests the corresponding Steam operation. A subscribed item remains distinct from an installed item, and installed content remains distinct from a loaded profile. Closing a download view stops observation where native cancellation is unavailable; Steam can continue downloading.

Use **Enable late** for an eligible client/shared profile and **Startup enable** for the game's next normal native loading path. These selections have separate persisted states. Apply or discard pending Options edits before changing either selection. **Load new profiles** restores eligible multiplayer client/shared content; repeated refresh preserves loaded module objects and callbacks. [Runtime behavior](docs/runtime.md) defines native-startup, missing-server-code, ordering, and lifecycle limits.

**Reload a profile** requires explicit upstream cleanup cooperation. LuaBuddy compiles and validates the replacement before calling the running generation's captured cleanup function. A refusal retains the old generation. Uncertain cleanup or failed activation quarantines it and requires reconnecting. Bundled profiles do not gain safe reload simply because they load successfully.

## Select optional integrations

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
| [True Line of Sight](https://steamcommunity.com/sharedfiles/filedetails/?id=3792740760) | `TrueLineOfSight` | Mount its fallback outline image and restore saved preferences; use upstream player and render callbacks without startup replay. |
| [Batch Craft Recipe](https://steamcommunity.com/sharedfiles/filedetails/?id=3584890848) | `vac_mod_b42_10` | Use upstream right-click batch modes, native crafting actions, and craft-recipe tag changes without a custom hook. |
| [Nutrition Helper UI](https://steamcommunity.com/sharedfiles/filedetails/?id=3803702351) | `NutritionHelper` | Restore saved preferences and attach the Health-tab button; load before AllTheInfo when both are selected so both food-tooltip renderers remain available. |
| [Quick Build Menu](https://steamcommunity.com/sharedfiles/filedetails/?id=3741839491) | `QuickBuildMenu` | Preload the native build action, restore saved options, and preserve handcraft favorites during upstream startup cleanup. |
| [NeatUI Mechanics](https://steamcommunity.com/sharedfiles/filedetails/?id=3832114824) | `NeatUI_Mechanics` | Requires `NeatUI_Framework`; mount images, restore preferences, and use upstream setup for existing mechanics windows and sidebars while preserving current window dimensions. |
| [Better Crafting](https://steamcommunity.com/sharedfiles/filedetails/?id=3618123324) | `EURY_CRAFTING` | Restore and apply saved preferences; use upstream recipe browsing, XP tooltips, sorting, ingredient memory, and favorites. |
| [Banger's Retro Car Dashboard](https://steamcommunity.com/sharedfiles/filedetails/?id=3739256322) | `RetroDashboard` | Mount its images and use the upstream dashboard factory for existing player UI when loading during play. |
| [Mod Manager Reforged (Client)](https://steamcommunity.com/sharedfiles/filedetails/?id=3737874810) | `MMReforgedC` | Mount images and run its missed boot patch registration. Enable it normally in the game's Mods menu for the main-menu manager. |
| [NeatUI Framework](https://steamcommunity.com/sharedfiles/filedetails/?id=3508537032) | `NeatUI_Framework` | Optional dependency profile for NeatUI Inventory and Mechanics; mount the framework's UI images. |

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

An offline build needs the locked dependencies and advisory database already cached. [Verification](docs/verification.md) describes fetching those development inputs, evidence, exact proof scope, and separate graphical and performance gates. The [behavior ledger](docs/evolution/behavior.md) maps the old functionality to its Rust replacement. The [original evolution request](docs/evolution/request.md) remains the acceptance source.
