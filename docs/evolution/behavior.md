# Rust behavior ledger

The acceptance source is [the original request](request.md). The migration baseline is commit `84d495a`, with 24 optional profiles and 16 executable Lua integration hooks. Its passing old aggregate is historical evidence only. The replacement maps each behavior to Rust production code and independently observable checks.

## Loader and lifecycle parity

| Behavior | Rust owner | Verification boundary |
| --- | --- | --- |
| Fresh selection is empty; arbitrary IDs preserve Unicode and portable filenames. | `luabuddy-core::profile`, Rust installer, UI persistence. | Real catalog consumer, installer CLI journeys, native profile parsing, graphical persisted selection. |
| Java properties escaping, continuations, booleans, duplicate IDs, and precise legacy-hook refusal. | Core profile parser and integrity-checked migration catalog. | Core contracts, archived original-hook migration, invalid custom hook native consumer, graphical continued-property preservation. |
| Native discovery/version/dependency metadata, registered aliases, flat Workshop fallback and cache recovery. | Native game adapter. | Actual game/JVM metadata consumer. |
| Common/version precedence, shared/client separation, normalized aliases, server/translation source exclusion and deterministic glob filtering. | Core source scanner and module registry. | Actual production runtime fixtures and source-policy contracts. |
| Native requirements plus optional before/after/preloads; missing requirements/cycles; native-active reciprocal order. | Core resolver and session preflight. | Actual native runtime ordering and failure-isolation cases. |
| All shared phases precede clients; hooks and scoped boot replay finish before dependent clients. | Rust runtime pipeline. | Actual production phase/replay consumer. |
| Native require fallback, nil/false cache, table/closure identity, shadowed source identity and once-per-generation execution. | Owned module registry and Rust callable resolver. | Actual Kahlua calls through the production library, including late imports. |
| Multiplayer client restoration, dedicated-server isolation, initial menu management and fresh reconnect. | Runtime lifecycle, native UI and exact dispatcher hooks. | Actual event consumers plus separate graphical menu and multiplayer evidence. |
| Startup/early UI fallback, owner-thread timing, queued/nested dispatch and ready state. | Exact native dispatcher adapter. | Bundled JVM verifier/JNI proof and actual runtime guards. |
| Refresh adds only new selections; no-op retains resolver, module and UI identity. | Runtime/session policy and native Options rebuilding. | Actual refresh/no-op consumer and graphical native controls. |
| Prepare/compile replacement before cleanup; reject incompatible dependencies, order, paths, aliases and old/new shared filenames. | Core lifecycle policy and runtime preflight. | Native production reload and independent ownership counterexamples. |
| Explicit upstream cleanup, original context, false refusal, true replacement and uncertain quarantine. | Bounded cleanup metadata and captured callable. | Actual production cleanup scenarios, retained loaded-record diagnostics and failure-without-retry checks. |
| Callback ownership, enumeration, verified removal and scoped replay preserve foreign callbacks. | Native game callback adapter and generation ownership. | Actual callbacks, removal refusal, registration membership and owner/dependent replay failure. |
| Native translations, language fallback and placeholders. | Existing host parser/formatter through Rust adapter. | Actual common/version, English/German/fallback and substitution consumer. |
| Opt-in mounting preserves native server IDs and cannot claim every native asset. | Native filesystem adapter plus explicit profile intent. | Native metadata/consumer checks and integration-specific graphical assets. |
| Live eligibility and future native selection remain separate. | Shared activation policy, explicit `live` metadata, and native default-selection adapter. | Final native preference roundtrip and graphical checks must confirm future selection without mutating the current world/server. |
| Pending Options, keyboard dismissal, modal focus, repeated opening and persisted controls. | Rust UI through existing native factories. | Actual isolated graphical client. Controller hardware needs its own evidence. |
| Stable sessions, source/stage/configuration metadata, Lua/Java traces and truthful failure states. | Core diagnostics, native report adapter, CLI inspection. | Actual written reports, rejected-generation retention and malformed/stale/failed CLI report checks. |
| Preserve launcher flags/configuration, profiles, user edits and shared legacy consumers through install/update/repair/rollback/uninstall. | Rust installer, immutable generations and transaction journal. | Outside-in CLI fixtures, interrupted-prefix recovery, exact migration, Unicode paths and real launcher activation. |

The native adapter refuses unsupported game layouts and unknown defining loaders. It does not bypass checksums, server restrictions, or anti-cheat. A compiled source is not evidence that arbitrary third-party side effects can be rolled back.

## Bundled integration inventory

These identifiers come from the actual `profiles/*.properties` catalog. Only explicitly selected profiles activate. `profiles/hooks.rs` supplies all 16 compiled Rust hook replacements. No bundled profile declares cleanup merely because initialization succeeds.

| Profile ID | Rust hook | Declarative purpose |
| --- | --- | --- |
| `AllInfo` | `AllInfo` | Startup replay and conditional trait-screen exclusions. |
| `AreaTasks` | None. | Native ID-only initialization. |
| `AutoAll` | `AutoAll` | Native mechanics/pathfinding preloads and startup replay. |
| `AutoFishing` | None. | Opt-in native file mounting. |
| `CraftPlanner` | `CraftPlanner` | Optional Neat Crafting order, mounting and boot replay. |
| `EURY_CRAFTING` | `EURY_CRAFTING` | Restore and apply upstream preferences. |
| `KADropnLoot` | `KADropnLoot` | Optional NeatUI order and preferences. |
| `LootSenseQuickStack` | `LootSenseQuickStack` | Restore upstream preferences. |
| `LootSenseStorage` | `LootSenseStorage` | Startup replay and preferences. |
| `LootTrackerB42` | `LootTrackerB42` | Mounting, startup replay and preferences. |
| `MMReforgedC` | None. | Mounting plus scoped boot/start replay. |
| `NeatUI_Framework` | None. | Optional native dependency and image mounting. |
| `NeatUI_Inventory` | `NeatUI_Inventory` | Optional wrapper order, mounting and existing-page reconstruction. |
| `NeatUI_Mechanics` | `NeatUI_Mechanics` | Mounting and existing mechanics UI/sidebar adaptation. |
| `NutritionHelper` | `NutritionHelper` | Optional AllInfo order, startup replay and preferences. |
| `P4AlarmSyndrome` | `P4AlarmSyndrome` | Mounting, start/load replay and preferences. |
| `QuickBuildMenu` | `QuickBuildMenu` | Native action preload, preferences and startup favorite preservation. |
| `ReloadAllAmmo` | None. | Native source behavior and icon mounting. |
| `RetroDashboard` | `RetroDashboard` | Mounting and upstream dashboard factory adaptation. |
| `SuperOl3gContextMenuIcons` | None. | Optional companion order, mounting and startup replay. |
| `TrueLineOfSight` | `TrueLineOfSight` | Mounting and upstream preferences. |
| `vac_mod_b42_10` | None. | Native ID-only initialization. |
| `WalkLootForage` | `WalkLootForage` | Flat Workshop fallback, startup replay and preferences. |
| `WhatItDoes` | None. | Scoped startup initialization. |

Recount the current catalog through `luabuddy profiles --profiles-dir profiles`. The package's migration catalog records the exact original hook digests without retaining executable compatibility payloads in the release package. Custom or modified hooks are preserved and refused precisely until their behavior has a supported Rust or bounded declarative replacement.

## Added product capabilities

| Capability | Owner | Evidence and remaining limits |
| --- | --- | --- |
| Stable patch IDs/ownership, exact targets/fingerprints, dependencies/priority, conflicts and receipt diagnostics. | Reusable patcher registry and JVM backend. | Public SDK contracts and actual unrelated JVM targets. |
| Two independent compiled Rust extensions through a versioned C ABI. | Patcher extension boundary and example crates. | Actual binary loading and live JVM replacement/observation; explicit approval and full OS privilege contract. |
| Native Kahlua callbacks without executable foreign helpers. | Declaration-only native bridge and weak callable ownership. | Real arguments/returns/errors/reentrancy, native UI callback and classloader isolation proof. |
| In-game installed discovery and initialized Workshop search/details. | Native UI and Steam adapter. | Actual isolated graphical read-only workflow. Per-item unavailability stays distinct from healthy results. |
| Subscription/download controls and observed progress/failure. | Item-scoped intent, native callbacks and owner-thread ticket polling. | Native notification proof is account-free. New account mutation testing remains explicitly blocked by the user's choice. |
| Native packaging, integrity manifests, migration and recovery. | Rust CLI and verification driver. | Real launcher plumbing plus disposable installer and interrupted-write scenarios. |
| Subscribe-only one-click Workshop activation. | Host/platform activation boundary. | Unresolved. Delivered native content has no demonstrated automatic activation action in the inspected Workshop loading path. |
| Windows/macOS live execution and matched game performance. | Platform/release verification. | Not established by portable CI or native micro proofs. |

A final aggregate must run against the final artifact. Graphical gameplay, hardware input, account permissions, native platform availability, and performance retain their own passed/failed/not-run entries. This ledger does not convert an unrun acceptance gate into a pass.
