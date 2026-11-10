# LuaBuddy Rust evolution plan and implementation prompt

LuaBuddy evolves in place into a Rust-native mod management and patching platform. Its own general-purpose patcher replaces the ZombieBuddy dependency, while its in-game interface lets players find, add, manage, and load mods. One-click Workshop setup is a product requirement to prove against the actual game and Steam environment.

The plan below describes the architecture and delivery gates. The final section is the complete standalone prompt to give an implementation agent. No implementation is performed by this document.

# Part One Architecture and delivery plan

## 1 Product outcome

One product, LuaBuddy, owns:

- A Rust runtime and reusable general-purpose patching engine.
- Project Zomboid integration with no ZombieBuddy dependency.
- In-game browsing, searching, adding, downloading, enabling, loading, and managing mods.
- Safe runtime refresh and reload where the affected mods support them.
- A one-click Workshop installation and activation experience wherever the host platform permits it.
- Reversible installation, configuration migration, diagnostics, and updates.

The patching engine is part of LuaBuddy’s architecture and public developer surface. It is not an unrelated replacement product. General-purpose means reusable patch capabilities and extension points, rather than hard-coded support for a pair of startup events. Game-specific integration belongs in a narrow adapter.

The implementation is Rust. Existing game classes and third-party Workshop Lua mods remain host content. LuaBuddy does not embed another interpreter or move its own implementation into foreign-language strings, generated helpers, or executable payloads.

## 2 Prove the difficult boundaries early

Three investigations run together before the implementation depends on an unproven route.

### Native integration and callable behavior

Prove that Rust can reach the actual game/JVM lifecycle, participate in event dispatch, interact with Kahlua values and callables, track callbacks, and implement the UI and loader behavior.

JNI access alone is insufficient evidence. The game’s existing Lua runtime expects particular callable objects, and lifecycle events can be queued or dispatched under locks. Verify actual dispatch timing, owner-thread execution, reentrancy, return values, identity, and exceptions.

Investigate existing host APIs and narrow native JVM facilities first. A Rust-native JVM agent is a candidate, not a predetermined implementation. Measure the overhead of the chosen hook mechanism.

The strict-language boundary applies to generated code too. A Java or Lua helper does not become Rust because Rust produces it. If a required callback or patch mechanism needs executable non-Rust glue, present its exact role and the constraint it resolves before changing the architecture.

### Workshop installation and activation

Trace the complete path from a clean, unconfigured game installation to an active LuaBuddy runtime:

1. The player subscribes to or installs the Workshop item.
2. Steam delivers the appropriate content.
3. The game’s supported loading path activates the correct native runtime.
4. LuaBuddy’s in-game interface becomes available.
5. Updates and uninstall leave the game usable.

Distinguish delivery from activation. A native library inside a Workshop folder is not proof that the game loads it. Test clean installation, ordinary launch, offline relaunch, update, and removal.

Do not assume one-click activation is impossible, and do not assume it works. Establish the actual loading facilities and restrictions. If the host cannot activate native code through the requested route, document the exact platform gate and the smallest additional activation step. Keep one-click setup as an unmet target rather than silently redefining it.

### In-game mod management

Prove a vertical slice inside Project Zomboid:

- Find a Workshop or installed mod.
- Inspect its identity, dependencies, and compatibility information.
- Add or subscribe to it through the supported Steam/game APIs.
- Show download progress and failure states.
- Enable and load it when safe.
- Explain and arrange a restart or reconnect when immediate loading is unsafe.

The interface must distinguish subscribed, downloading, installed, enabled, loaded, disabled, failed, and restart-required states. “Subscribed” is not equivalent to “loaded.” Steam supports browsing, subscriptions, and downloads, but the actual game build’s exposure of these APIs must be verified rather than inferred from documentation for an older build. Record a capability matrix for native bootstrap, Workshop access, download, activation, and live loading on each supported platform.

## 3 Internal architecture

Use a small Rust workspace organized by responsibility.

### Core and lifecycle

Own profile data, dependency resolution, module/cache state, ordering, lifecycle, generation tracking, refresh/reload policy, and structured diagnostics. Keep raw JNI and operating-system assumptions out of this core.

### General-purpose patch engine

Provide a reusable Rust API and patch registry with:

- Stable patch IDs, ownership, targets, dependencies, and deterministic priority.
- Exact target matching and compatibility preconditions.
- Before/after hooks and controlled argument, result, replacement, or around behavior where the proven host mechanism supports them.
- Explicit semantics for cancellation, exceptions, reentrancy, and nested calls.
- Conflict detection, duplicate prevention, and capability reporting.
- Safe enable/disable and rollback where the mechanism can actually guarantee them.
- Scoped diagnostics showing installed, rejected, failed, and disabled patches.

Implement and test advertised capabilities. Expose unsupported operations honestly rather than a nominal API backed by no-ops.

Keep hook logic in Rust and patch metadata in a fixed declarative schema. Offer a documented Rust SDK and compiled Rust extensions through a versioned C ABI; this specifies a binary boundary, not a C-language implementation. Validate privileges, provenance, ABI compatibility, and explicit install/enable intent before activation. Downloading unrelated Workshop content does not grant blanket permission to execute native libraries. Prove generality with several unrelated patch targets and at least two independent Rust extensions.

Distinguish edits to existing host bytecode from embedded application logic. Evaluate structural host-method transformations as a possible patch mechanism, implemented by Rust and tested against the JVM verifier. Generated helper classes or foreign-language algorithms remain outside the strict Rust boundary. Specify any proposed inserted instructions and their role explicitly rather than hiding implementation in a bytecode payload.

### Game and JVM adapter

Own class/member resolution, JVM capability negotiation, JNI reference lifetimes, game-thread scheduling, Kahlua interaction, event ownership, game UI access, and supported Steam/Workshop integration.

Keep private API dependencies visible and narrow. Detect incompatible changes before modifying state. Rust improves maintainability but cannot make private game interfaces permanently stable.

### In-game interface

Provide discovery and search, details and dependencies, add/install, download state, enable/disable, load/reload, updates, diagnostics, and recovery flows. Preserve keyboard/controller input, focus, cancellation, modal behavior, and existing options integration.

A Rust-owned UI may use existing game facilities. Any need for new Lua/Java callback implementation is part of the strict-language feasibility gate.

### Installation and release support

Keep platform discovery, packaging, launch integration, migration, integrity checking, repair, rollback, and uninstall within LuaBuddy. A helper executable can be part of this same product if necessary, but must not be assumed before investigating Workshop activation.

## 4 Preserve existing behavior and migrate extensions

Derive the complete behavior ledger from the current checkout and its tests. Cover:

- Shared/client sources, normalized paths, aliases, and precedence.
- Dependencies, cycles, deterministic ordering, required/before/after/preload behavior.
- Native loader fallback, cached nil/false, identity, and execution once per generation.
- Startup, menu, new-world, reconnect, and repeated initialization.
- Options persistence and UI behavior.
- Refresh without rerunning loaded mods.
- Validation and compilation before disrupting a running generation.
- Reload eligibility, cleanup consent, refusal, and uncertain cleanup.
- Ownership-scoped callback removal and event replay.
- Diagnostics and reconnect-required outcomes after partial mutation.

Port every bundled profile and hook, using the actual inventory rather than a fixed historical count.

Handle custom Lua hook files explicitly: preserve them, detect references, identify their behavior, and migrate supplied logic to Rust or bounded declarative data. Arbitrary executable Lua extensions are not silently retained under a full-Rust claim. An unmigrated hook receives a precise error and migration guidance.

## 5 Safe loading and patching

Loading an arbitrary mod during play is not universally safe. Separate operations the runtime can complete immediately from those requiring a new session, reconnect, or restart.

Resolve dependencies and ordering before activation. Track module and callback ownership by generation. Prepare replacements before cleanup. Preserve a working generation when cleanup refuses; quarantine an uncertain generation when cleanup partially changes state.

Do not claim general rollback of arbitrary third-party side effects. Protect saves, other mods, and unrelated callbacks. Respect multiplayer authority, server mod policy, integrity protections, and permissions.

General patching capability does not authorize bypassing anti-cheat or server restrictions.

## 6 Hermeticity and platform support

Use a pinned Rust toolchain, lockfile, audited dependencies, reproducible packaging, a documented offline build route, and versioned manifests. LuaBuddy should not require Python, a runtime compiler, an extra interpreter, or uncontrolled dependency downloads.

The game, JVM, Steam client, and OS remain external host dependencies. Do not redistribute proprietary game files.

Keep installation paths, Steam libraries, launch integration, and native artifacts platform-specific behind stable interfaces. Verify actual JVM process architecture rather than inferring it from the host CPU.

Test Windows, Linux, and macOS targets that the release claims to support. Cross-compilation is build evidence, not proof that the game runs successfully.

## 7 Installation and update safety

Preserve user configuration, saves, other mods, Workshop subscriptions, and unrelated launch settings. Record owned changes. Support interrupted/repeated installation, migration, update, repair, rollback, and uninstall.

Remove LuaBuddy’s ZombieBuddy dependencies and owned integration. Preserve a shared ZombieBuddy installation if other mods still require it.

Prefer supported loading and launch mechanisms that survive Steam updates. Do not modify Steam-managed game binaries or JARs as the default installation method.

Use disposable game-user directories and copied saves for testing.

## 8 Delivery gates

1. **Native proof:** Rust hooks, callable behavior, lifecycle, and UI integration work through the chosen mechanism.
2. **Workshop proof:** a clean-install test establishes the actual one-click activation path or an exact platform gate.
3. **In-game proof:** browse, add, download, enable, and load work end to end with safe restart/reconnect handling.
4. **Migration:** all existing supported behavior and bundled hooks have Rust replacements and tests.
5. **Patcher:** the reusable API, conflict management, advertised hook capabilities, and diagnostics pass independent tests.
6. **Release:** installation, update, rollback, native platform tests, performance, and independent review are complete.

Do not treat an agent-load message, successful build, downloaded archive, or one working profile as completion.

## 9 Verification and evidence

Run formatting, linting, unit tests, integration tests, dependency/security checks, and the repository’s aggregate verification.

Test invalid profiles, dependency cycles, callback identity, nil/false cache results, ownership, repeated initialization, cleanup failure, patch conflicts, target signatures/fingerprints, unsupported layouts, and concurrent/nested hook behavior. For supported bytecode transformations, verify stack maps, exception tables, return behavior, class verification, and interaction with other patches.

Exercise in-game UI repeated actions, interrupted downloads, failed subscriptions, missing dependencies, cancellation, keyboard/controller focus, restart-required transitions, and resuming after relaunch.

Test installation on a clean configuration, upgrades, interrupted updates, rollback, uninstall, spaces/Unicode paths, multiple Steam libraries, and user-modified files.

Compare matched baseline and enabled runtime performance. Fix the workload and budget before deciding results; record startup time, frame-time percentiles, CPU, memory, and growth across reload/reconnect cycles.

Report passed, failed, and not-run checks separately. Name exact versions and platforms in test evidence, not as permanent assumptions in the implementation prompt.

## 10 Sources

The following sources explain existing behavior and relevant host constraints. They support the plan; the execution prompt is complete without them.

- [Repository](https://github.com/DevGuyRash/LuaBuddy)
- [Startup integration](https://github.com/DevGuyRash/LuaBuddy/blob/84d495a288610de2a07a8685474cebb91eb6c1a3/src/luabuddy/Patches.java)
- [Loader and reload behavior](https://github.com/DevGuyRash/LuaBuddy/blob/84d495a288610de2a07a8685474cebb91eb6c1a3/resources/bootstrap.lua)
- [Options and UI behavior](https://github.com/DevGuyRash/LuaBuddy/blob/84d495a288610de2a07a8685474cebb91eb6c1a3/resources/options.lua)
- [Profile catalog](https://github.com/DevGuyRash/LuaBuddy/tree/84d495a288610de2a07a8685474cebb91eb6c1a3/profiles)
- [Verification documentation](https://github.com/DevGuyRash/LuaBuddy/blob/84d495a288610de2a07a8685474cebb91eb6c1a3/docs/verification.md)
- [JVMTI specification](https://docs.oracle.com/en/java/javase/25/docs/specs/jvmti.html)
- [Project Zomboid guidance on Java mods and modding](https://projectzomboid.com/blog/news/2026/10/a-note-on-java-mods-and-modding/)

# Part Two Complete standalone implementation prompt

Evolve this project in place into a complete Rust-native mod management and general-purpose patching platform. LuaBuddy remains the single product: its own patcher, runtime, in-game interface, Workshop integration, and installation support replace its ZombieBuddy dependency and existing non-Rust implementation.

Implement the work through testing, independent review, packaging, and delivery. Continue beyond scaffolding, a proof of concept, or an intermediate milestone. Use `@pstack:poteto-agent` when available; otherwise use the capable agents and tools present and identify unavailable tooling accurately. Parallelize independent work without losing ownership of integration.

## 1 Required product capabilities

Deliver:

- An idiomatic Rust implementation of the existing supported functionality.
- A capable general-purpose Rust patcher within the project, with a documented reusable API.
- No dependency on ZombieBuddy for building, installation, execution, or updates.
- An in-game interface to browse/search, inspect, add, download, enable, load, disable, update, and manage mods.
- Runtime refresh/reload where safe, with clear restart/reconnect paths where required.
- One-click installation and activation through Steam Workshop when the game/platform supports that route.
- Durable compatibility checks, reproducible builds, platform-aware packages, diagnostics, repair, rollback, and uninstall.

Read the current checkout, repository instructions, tests, and relevant installed game APIs. Establish the actual game/JVM/Steam environment rather than assuming a fixed version. Use repository-relative paths in your work and documentation. Preserve unrelated user changes.

## 2 Rust implementation boundary

Implement all first-party runtime, installer, patch hooks, profile behavior, and application logic in Rust. Declarative configuration and metadata are allowed.

Use the game’s existing Java classes and Kahlua runtime as host facilities. Existing third-party Workshop Lua mods may execute through the game’s own runtime; those mods are content the application manages, not a substitute implementation for this project.

Do not embed another interpreter or implement project behavior using Lua, Java, Python, JavaScript, Wasm, or another language hidden in strings, templates, byte arrays, generated source, executable resources, or helper programs. Generated JVM classes or bridge bytecode do not automatically satisfy the Rust requirement.

Keep patch logic in Rust and metadata declarative. Inspecting host code is not the same as authoring a foreign-language implementation. If a necessary mechanism requires executable non-Rust glue, identify the exact code role and limitation, demonstrate why the compliant alternatives fail, and request the smallest architecture decision. Do not silently relax the language boundary or claim reduced functionality is complete.

## 3 Prove the integration and Workshop path

Build focused executable proofs using disposable game-user directories and copied saves.

For native integration, prove:

- Native startup through the actual supported host mechanism.
- Correct classloader, class/member resolution, signatures, capabilities, and version checks.
- Hooks at the actual execution/dispatch boundary, including queued events and exceptional returns.
- Owner-thread execution, reentrancy, locks, and nested-call semantics.
- Required callable interaction, module resolution, cache behavior, and identity.
- Callback ownership, enumeration, removal, and lifecycle isolation.
- A representative in-game UI action and profile behavior.
- Acceptable measured overhead and safe rejection of incompatible layouts.

JNI access alone does not create a Kahlua callable. Inspect the actual runtime types and callable contracts, including event callbacks and require/UI operations. Verify rather than assume that a method-entry/return pair brackets completed game-event dispatch.

Investigate host APIs and narrow native JVM facilities first. Choose the mechanism from evidence. Do not ship global method tracing as an unmeasured production default.

For one-click Workshop setup, trace and test the entire clean-install route: subscribe/install, download, native activation, ordinary game launch, available in-game interface, update, offline relaunch, and uninstall. A library delivered in a Workshop directory is not proof that it loads.

Treat one-click setup as an acceptance target. Investigate all supported native loading and Workshop facilities before selecting the packaging design. Build a per-platform capability matrix covering native bootstrap, Workshop browse/subscribe/download, activation, and live loading. Verify the current game’s Steam API exposure directly rather than inferring it from older-build documentation. If a platform gate prevents automatic activation under the Rust constraint, report the exact missing capability and smallest additional activation step. Do not silently substitute a manual installer or claim that downloading alone meets the target. Continue independent implementation while the decision is resolved.

## 4 Build the internal architecture

Organize a small Rust workspace around:

- Core profile, dependency, module/cache, lifecycle, generation, and diagnostics logic.
- The reusable patching engine and Rust extension API.
- A narrow game/JVM adapter.
- The in-game interface and mod management workflow.
- Installation, migration, packaging, updates, and verification.

Keep game-specific assumptions and unsafe JNI/JVMTI operations out of the core. Avoid unnecessary services and unrelated products. Helpers, if needed, remain components of this application and share its release/configuration lifecycle.

Make the patcher general-purpose within the host integration, rather than hard-coding only the project’s initial event hooks. Provide stable patch IDs, owners, target descriptors, dependencies, priority, compatibility preconditions, conflict detection, and diagnostics.

Implement before/after hooks and controlled argument/result, replacement, or around capabilities where the proven mechanism supports their semantics. Define cancellation, exception, nesting, and reentrancy behavior. Every advertised capability needs executable tests; reject unsupported operations explicitly.

Support safe activation/deactivation and rollback where guaranteed. Detect conflicting ownership and stale targets using signatures/fingerprints before modifying state. Deliver a documented Rust SDK, fixed-schema declarative metadata, and compiled Rust extensions through a versioned C ABI. The ABI is a binary contract, not a requirement to write C. Validate extension privileges, provenance, compatibility, and explicit install/enable intent; do not silently execute arbitrary downloaded native code. Prove generality with several unrelated targets and at least two independent extensions.

Evaluate structural edits to existing host bytecode separately from embedded helper code. Keep patch policy and hook behavior in Rust. Specify every supported transformation and the role of inserted instructions; do not place foreign-language application algorithms or generated helper classes in the runtime as a workaround. Test verifier validity, stack maps, exception tables, return/exception semantics, and coexistence with other patches. Escalate any actual conflict with the strict language boundary rather than assuming all bytecode edits are either forbidden or automatically compliant.

## 5 Preserve and migrate existing functionality

Inventory the current source, profiles, hooks, commands, and tests. Create a behavior ledger linking each supported behavior to its replacement and verification. Port the actual complete inventory rather than using a hard-coded historical count.

Preserve:

- Shared/client discovery, normalized paths, aliases, and precedence.
- Required/before/after/preload ordering, deterministic resolution, and cycle detection.
- Native loader fallback, cached nil and false, object/closure identity, and once-per-generation execution.
- Startup, menu, world creation, reconnect, and repeated initialization.
- Options persistence and UI behavior.
- Loading newly enabled profiles without rerunning loaded ones.
- Compilation/validation before disturbing a working generation.
- Reload eligibility, explicit cleanup consent, refusal, and uncertain cleanup handling.
- Removal and replay of only the relevant owned callbacks.
- Rejection of unsafe reloads involving dependencies or shared sources.
- Actionable source/stage/Lua/JVM diagnostics.

Port every bundled hook to Rust. For custom Lua hook references, preserve the files, detect and explain the dependency, and migrate supplied behavior to Rust or bounded declarative data. Report unmigrated hooks specifically. Do not ignore them or execute them as a hidden compatibility workaround.

Remove obsolete ZombieBuddy and non-Rust implementation paths after their replacements and migration tests are ready. Preserve necessary license/attribution records and useful history.

## 6 Implement in-game mod management

Build a usable interface inside Project Zomboid for:

- Browsing/searching installed, subscribed, and available Workshop mods through supported APIs.
- Viewing verified identity, descriptions, dependencies, available version/compatibility information, and source.
- Adding/subscribing/downloading with progress, cancellation where supported, retry, and clear failures.
- Enabling/disabling with dependency and load-order resolution.
- Loading newly added content during play when the runtime can do so safely.
- Reloading cooperating mods safely.
- Handling updates, restart/reconnect requirements, and resuming after relaunch.
- Viewing patch/mod state, diagnostics, and recovery actions.

Represent subscribed, downloading, installed, enabled, loaded, disabled, failed, and restart-required as distinct states. Do not show success until the corresponding operation is verified.

Handle repeated clicks, interrupted downloads, missing dependencies, unavailable Workshop access, pending settings changes, keyboard/controller focus, modal dismissal, and repeated opening/closing.

Use existing game UI facilities where suitable, with first-party behavior implemented in Rust. Any callable limitation remains part of the integration gate.

Do not promise that every third-party mod can load live. Determine eligibility before activation, explain why a restart/reconnect is necessary, and provide the smoothest supported continuation.

## 7 Protect state and respect game authority

Track modules, callbacks, patches, and references by owner and generation. Prepare replacements before cleanup. Retain the working generation when cleanup refuses; quarantine uncertain partial changes and explain when reconnect is required.

Do not claim rollback of arbitrary third-party side effects. Preserve saves, unrelated configuration, other mods, and foreign callbacks.

Respect multiplayer authority, server policies, mod validation, checksums, and anti-cheat. A general-purpose patching API must not be used to bypass these protections.

Use disposable profiles and copied saves for development. Use authorized test mods and environments; implementing subscription controls does not authorize arbitrary live subscriptions on the user’s account.

## 8 Make distribution reversible and reproducible

Keep Steam and Workshop discovery, launch behavior, updates, and mod identities working.

Implement install, verify, migration, repair, diagnostics, update, rollback, and uninstall. Preserve unrelated JVM flags, user edits, Workshop data, and other mods. Track owned files/settings and recover from interrupted or repeated operations.

Remove only this application’s ZombieBuddy dependency and owned integration. Preserve a shared installation still used by another mod.

Prefer supported mechanisms that survive normal Steam updates. Do not modify Steam-managed JARs or executables as the default installation route.

Pin the Rust toolchain and dependencies. Provide a lockfile, audited dependencies, versioned manifests, artifact integrity checks, and a documented offline build route. Avoid runtime compilation, extra interpreters, and uncontrolled dependency downloads.

Treat the game, JVM, Steam, and OS as external dependencies. Do not redistribute proprietary game files.

Package for the actual supported OS/JVM architectures. Verify Windows, Linux, and macOS combinations before labeling them supported. Mark unavailable combinations unverified rather than treating cross-compilation as a live pass.

## 9 Verify safety and completion

Handle JNI reference lifetimes, thread attachment, classloaders, JVM exceptions, locks, reentrancy, and generation cleanup explicitly. Prevent Rust panics from unwinding across FFI. Keep unsafe code small and reviewed.

Detect incompatible runtime layouts before activation. Provide useful diagnostics and a safe inactive path rather than speculative patching.

Run formatting, linting, unit/integration tests, dependency/security checks, and the repository’s aggregate checks. Include:

- Behavior parity and every bundled hook.
- Native callable/dispatch/lifecycle behavior.
- Patcher capabilities, conflicts, nesting, exceptions, and incompatible targets.
- In-game browse/add/download/enable/load and restart-required workflows.
- Interrupted/repeated UI operations and keyboard/controller behavior.
- Invalid configuration, missing dependencies, cycles, and aliases.
- Cleanup refusal/failure, reconnect, and repeated initialization.
- Clean install, upgrade, interrupted update, repair, rollback, and uninstall.
- Spaces/Unicode paths, multiple Steam libraries, and user-modified files.
- Workshop clean-install activation and ordinary Steam launch.
- Authorized multiplayer scenarios before making multiplayer claims.
- Artifact checks for retained ZombieBuddy dependencies and prohibited implementation payloads.
- Live runtime checks for each claimed platform.

Measure matched baseline/enabled performance using a fixed workload and a budget chosen before the deciding results. Record startup, frame-time percentiles, CPU, memory, and growth across reload/reconnect cycles.

Use independent review of Rust/FFI correctness, behavioral parity, patching safety, UI workflows, installation, and release contents. Fix findings and rerun affected checks against the final code.

## 10 Deliver the finished result

Deliver complete source and scoped commits, release packages and integrity manifests, installation/migration/update/rollback/uninstall instructions, patcher API documentation, mod/profile migration guidance, a behavior ledger, and a concise verification report.

The report must identify what works, exact tested versions/platforms, passed/failed/not-run checks, independent review findings and resolutions, and any remaining limitation.

Honor applicable task-specific authorization for push, publication, Workshop upload, merge, and deployment. Carry out expressly authorized actions after required checks, subject to safety requirements. If authorization for a particular action or destination is absent, finish the local deliverables and request only what is missing.

Completion means the requested application capabilities are implemented and verified, with the one-click Workshop target either demonstrated or identified as a specific unresolved platform/architecture gate. Do not label the overall request complete while a required capability remains unresolved.

If a required decision, permission, credential, or inaccessible environment is the remaining blocker, finish all independent work, state exactly what is blocked and why, and request the smallest missing input. Do not fabricate a test pass, hide an unmet target, or weaken the Rust boundary to avoid reporting a blocker.
