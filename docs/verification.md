# Verify a Rust checkout

Use the pinned Rust toolchain and committed lockfiles. The aggregate driver is Rust and does not require Python or a Java compiler. Native proofs read the installed game's proprietary classes in place and use its bundled JVM.

## Run the portable checks

Cache the declared dependencies before an offline run, then execute the portable aggregate.

```sh
cargo fetch --locked
cargo fetch --locked --manifest-path examples/extensions/math-guard/Cargo.toml
cargo fetch --locked --manifest-path examples/extensions/string-observer/Cargo.toml
cargo xtask check --portable --offline
```

The driver builds the complete application before running tests. It checks workspace formatting and strict Clippy, runs behavior and installer tests, compiles two independent Rust extensions, exercises their actual ABI consumer, validates the bundled profile catalog, creates and verifies a package from the current native artifact, and rejects prohibited implementation payload files in that package.

The driver snapshots the selected CLI and runtime bytes once after the application build. The default inputs are the completed debug artifacts. Commands and native JVMs use those immutable copies, even if another build later replaces the original files. The temporary copies and package are removed after inspection. Check transcripts and `report.json` remain under the printed evidence directory outside Git. The report records the exact artifact SHA256 values and native host identity. Supply `--log-dir` to choose a different evidence destination. A failed command, missing proof marker, proof `FAIL`, JNI warning, output limit, or deadline produces a failed aggregate result.

Portable CI runs on Linux, Windows, and macOS. Linux-only installer fixtures remain explicitly scoped. Portable success on another operating system is not a native game-runtime pass.

## Run the native proofs

Supply the selected installation directory, or let the installer identify one Steam game installation. An ambiguous or absent installation fails the native aggregate; use `--portable` when that narrower scope is intended.

```sh
cargo xtask check --offline --game-dir /path/to/ProjectZomboid
```

The driver selects `jre64/bin/java` from that installation, records its actual Java version and process architecture, and sets the proof invocation environment to that JVM. It does not infer JVM architecture from the machine CPU or fall back silently to `JAVA_HOME`.

Native crash diagnostics go into the evidence directory's `jvm-errors` folder. The proof environment disables core dumps, so a failed verification JVM does not place a core dump in the installed game directory. Evidence paths are resolved before child commands change working directories.

The currently observed Linux x86_64 single-JAR layout is the native proof target. Fetch the locked proof dependencies once before an offline run.

```sh
cargo fetch --locked --manifest-path proofs/metadata/Cargo.toml
cargo fetch --locked --manifest-path proofs/runtime/Cargo.toml
cargo fetch --locked --manifest-path proofs/steam/Cargo.toml
cargo fetch --locked --manifest-path proofs/patcher/Cargo.toml
cargo fetch --locked --manifest-path proofs/bridge-loader/Cargo.toml
cargo fetch --locked --manifest-path proofs/activation/Cargo.toml
cargo fetch --locked --manifest-path proofs/extensions/Cargo.toml
cargo fetch --locked --manifest-path proofs/extensions/marker/Cargo.toml
cargo fetch --locked --manifest-path proofs/performance/Cargo.toml
```

The native aggregate runs these distinct consumers.

| Consumer | What it exercises |
| --- | --- |
| Production `proof-native` mode | Real Kahlua callable identity, values, reentrancy, exceptions, scoped retirement, native UI mouse callback, synchronous/nested/queued owner-thread dispatch, unchanged original dispatcher bytecodes, boolean returns, Unicode event names, exceptional exits, and actual native Workshop notification routing without an account action. |
| `proofs/metadata` | Actual identity, dependencies, availability, native symlink and flat-root discovery, cache recovery, translation parser/fallback/placeholders, callback ownership/removal, and generated diagnostic contents. |
| `proofs/runtime` | The actual production library, copied immutably into a disposable fixture, through native events and its public Lua-facing operations. Covers cache/alias/phase behavior, refresh, reconnect, preflight, failure isolation, cleanup consent/refusal/quarantine, and scoped callbacks. Three additional fresh JVMs exercise one-launch claims, reconnect retention, a queue created after initialization, expiry, and unchanged persistent selections and source files. |
| `proofs/activation` | Actual native dependency metadata and preference serialization, explicit next-startup selection, preserved unrelated IDs and map order, unchanged current/server activation, and centralized live eligibility. |
| `proofs/steam` | Required native Steam symbols and rejection before an initialized host Steam context. No account mutation is performed. |
| The selected runtime's `proof-patcher` mode | Real unrelated JVM targets, both compiled Rust extensions, deterministic shared-hook order, mutation/replacement, original exceptions, reference identity, nesting/concurrency, stale targets, and exact disable. The standalone `proofs/patcher` consumer also builds. |
| `proofs/bridge-loader` | An unsupported classloader copy prepared during the transform-to-register interval retains its original declaration and does not poison native registration. The event dispatcher in another loader also retains its original declaration and bytecodes. |
| `proofs/extensions` | The selected normal product agent and CLI approval workflow across seven fresh JVM cases. Covers disabled and unapproved code, changed artifact/manifest, unchanged-binary metadata updates, approved native execution, and explicit retry after normal target preparation. |
| `proofs/performance` build and tests | The measurement program's ownership and provenance behavior. This does not run graphical workloads or establish a performance result. |

The driver applies deadlines and checks proof output in addition to process exit status. Its fixtures use disposable homes/caches and preserve the user's running game. No aggregate fixture subscribes to, unsubscribes from, or downloads a Workshop item. Explicitly authorized live account journeys are separate from this aggregate.

## Audit dependencies and packages

Install a reviewed `cargo-audit` development tool and keep a usable RustSec advisory database. Request the audit explicitly.

```sh
cargo xtask check --portable --audit
cargo xtask check --portable --offline --audit
```

The audit covers the workspace, extension, and proof lockfiles. Offline mode supplies `--no-fetch --no-yanked`; a missing or stale advisory database fails rather than becoming a security pass. Registry yanked-crate lookup is explicitly not run offline. Omitting `--audit` records the advisory check as not run.

To verify release artifacts, build them first and supply both paths. Add the package directory when that same release has been packaged.

```sh
cargo build --release --locked --offline
cargo xtask check --offline --audit --game-dir /path/to/ProjectZomboid --runtime target/release/libluabuddy_host.so --cli target/release/luabuddy --package dist/LuaBuddy-linux-x86_64
```

`--runtime` selects the exact library for native callable, patcher, runtime, and normal-product extension proofs. `--cli` selects the executable for discovery, profile inspection, packaging, and extension approval. Helper consumers for metadata, native preference APIs, and classloader interleaving compile from the current source and use the selected game's JVM. They are distinct evidence from execution of the selected library.

The installer verifies every manifest entry and the native artifact architecture. Entries beneath `profiles/` must be direct `.properties` templates. Nested entries and non-template catalog files are rejected. The aggregate also requires package runtime and CLI hashes to match the exact tested artifact snapshots. A release package paired with default debug inputs fails that identity check. To inspect package integrity alone, use `luabuddy verify-package --package DIR`. The aggregate additionally refuses symlinks and executable `.lua`, `.java`, `.class`, `.jar`, `.py`, `.js`, or `.wasm` payload files in the package. Current verification checks strict profile-option rejection and native installation behavior without a legacy conversion layer.

## Exercise graphical setup

Build the complete release and its verified native package first. Stage the Workshop download with `cargo xtask workshop --package DIR --out DIR`. This developer command requires GNU tar; it does not publish or install anything. Use a private desktop session, home, configuration, cache, and disposable game copy for the following checks. An inherited file-manager profile can restore unrelated tabs, and a sandboxed archive manager can hide `/tmp`; distinguish those verification conditions from package failures.

1. Open the staged setup archive through a file manager and extract it with the desktop archive manager. Confirm that the extracted `luabuddy` remains executable and its package passes integrity verification.
2. Double-click `luabuddy`, including from a working directory outside the package. Confirm that setup identifies the adjacent package and the selected disposable game. Cancel before installation and verify that no installation files changed.
3. Install through the GUI. Confirm an owned launcher override, a verified immutable generation, no automatic profile selection, and visible instructions to restart normally through Steam. Repeat installation and confirm truthful no-op feedback.
4. Apply another verified native package through graphical package selection, then test rollback and its cancellation. Repair changed owned bytes and verify that the recovery copy preserves them. Test explicit transaction recovery and automatic recovery followed by an otherwise idempotent action.
5. Test invalid packages, invalid user profiles, unsafe installation metadata, long paths containing markup characters, and many missing files. Valid ownership must remain distinguishable from incomplete inspection. Full diagnostics and action buttons must remain reachable. Remove through the GUI and verify that user profile bytes and unrelated launcher settings survive.
6. Repeat the supported dialog paths with Zenity and KDialog, including native folder selection, cancellation, and helper-wrapper invocation. Confirm that progress helpers exit and that no verification desktop or client remains running.

Keep screenshots, actual file/hash comparisons, and the exact CLI/runtime/package identities outside Git. CLI integration tests cover ownership and invocation gaps; they do not establish archive-manager extraction or desktop controls. A final authorized Steam journey must also confirm that the published archive downloads through Workshop and matches the verified package. Subscription alone does not activate the native runtime, and downloading an update does not switch the installed generation.

## Exercise the graphical product

Use disposable client caches, displays, profiles, and loopback servers. Keep the user's game and personal saves untouched. Match the exact native artifact SHA256 to each evidence set.

1. Install a clean package with no selected profiles through the real launcher override. Confirm the native manager appears after an ordinary launch.
2. Exercise installed discovery, details, search, unavailable Workshop, already subscribed read-only content, pagination, genuine per-item failures, and repeated open/close.
3. Drive actual controls with mouse and keyboard. Check focus, held Return, Escape, pending Options preservation, bounded dialogs, and controller navigation where hardware is available.
4. Type without submitting in each browse view. Confirm Installed filters cached rows immediately, network views debounce, active scope stays visible, and stale queries cannot replace current results. Keep subscription and download operations separate from observational query cancellation.
5. Check action groups, full workflow guides, upstream descriptions, long unbroken paths, large diagnostic reports, keyboard hints, reload picker selection, and custom entry. Verify actual selection files and runtime states after actions, including failures and startup-only refusals.
6. Add supported selections through actual native controls. Inspect saved preferences and profile contents, including Unicode names, continued properties, and file permissions.
7. Join an isolated ordinary-role multiplayer client to a loopback server with `Mods=`, `WorkshopItems=`, and `DoLuaChecksum=true`. Check affected integrations alone and with relevant Auto All/AllTheInfo combinations.
8. Cover initial restoration, reconnect, runtime addition, repeated no-op refresh, supported cleanup replacement, refusal, syntax failure, and uncertain cleanup. Inspect actual controls and saved state.
9. Stop only the owned client, server, and display. Remove their disposable caches. Retain selected logs/screenshots outside Git.

The aggregate does not launch that workflow automatically. Native unit/integration proofs and a graphical client establish different facts. Do not report the latter from the former.

The isolated aggregate performs no Steam account operations. A separate, explicitly authorized graphical workflow completed a fresh subscription and download for one test item through the final candidate; its scope is recorded in [Workshop evidence](evolution/workshop.md). That journey does not establish every item, an upstream content update, or subscribe-only native activation.

## Measure the final runtime

Choose a fixed workload and acceptance budget before collecting deciding baseline/enabled results. Alternate matched release-build runs. Record startup time, frame-time percentiles, CPU, memory, and growth across repeated reconnect/reload cycles. Verify that the changed work actually ran and count failures alongside timings.

A callback microbenchmark is not a game frame-time result. Cross-compilation is not platform gameplay evidence. Report passed, failed, and not-run checks separately, with actual game/JVM/OS versions, artifact hashes, and limits.

For a headless event-dispatch comparison, build `proofs/runtime` in release mode. Its optional measurement mode accepts the actual game directory, an immutable runtime library, `--event-benchmark`, and an iteration count between 1 and 1000000. Use the game's bundled JVM through `JAVA_HOME` and its native directory through `LD_LIBRARY_PATH`, as the aggregate does.

```sh
cargo build --release --locked --offline --manifest-path proofs/runtime/Cargo.toml --target-dir target/verify-runtime
target/verify-runtime/release/luabuddy-runtime-proof /path/to/ProjectZomboid /path/to/runtime.so --event-benchmark 100000
```

The measurement uses a disposable home, cache, and empty profile directory. It warms each case with 10000 calls, then checks every empty dispatch result and the completed Lua loop and callback counts. The direct empty case reaches `Event.trigger`; the other cases enter through the real Lua event API with a listener present. Setup and compilation stay outside the timed region. Normal runtime proofs retain `-Xcheck:jni`; measurement mode omits that instrumentation. Alternate at least five unchanged and candidate release runs and retain their printed artifact hashes, raw samples, errors, and machine load.

To record VM operations separately, append an absolute `.jfr` output path after the iteration count. That mode starts a narrowly configured JFR recording, then explicitly stops, dumps, and closes it before the invocation process exits. Exclude recorded runs from the timing comparison. The event measurements and recordings do not establish game frame times or the full performance budget. See [the event-dispatch result](evolution/event-dispatch.md).

## Evidence scope during the Rust migration

The current source accepts only the supported native profile format. Historical Java-agent and Lua-hook migration checks describe earlier implementations and do not define current compatibility.

The old implementation at commit `84d495a` passed its offline aggregate before replacement. That historical result is a baseline, not verification of Rust behavior. The Rust implementation run used Project Zomboid `42.22.2 3d39148846 integrity=pending`, bundled Azul Java `25.0.5+8-LTS`, and Linux x86_64.

The individual native consumers, compiled-extension execution, installer journeys, classloader regression, and graphical menu/read-only Steam checks have produced scoped evidence during implementation. The final release report must identify the deciding final artifact and the still-unrun account, platform, full-game, or performance gates. See [the behavior ledger](evolution/behavior.md) and [platform evidence](compatibility.md).
