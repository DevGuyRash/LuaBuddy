# Measure the native client

Use an owned vanilla loopback server and a disposable game-user directory. Keep `Mods=` and `WorkshopItems=` empty and `DoLuaChecksum=true`. An ordinary test player, fixed location, fixed viewport, and a stable scene are required. Do not use a personal save or account cache as the template.

Build the product and the Rust observer with the locked dependencies.

```sh
cargo build --release --locked --offline
cargo build --manifest-path proofs/performance/Cargo.toml --release --locked --offline
```

Create a private JSON file containing the supported native bootstrap property. Select the loopback port of the owned server.

```json
["-Dargs.server.connect=127.0.0.1:16461"]
```

Prime one disposable account through the game's native Multiplayer screen. Use `--account` to identify that fixture exactly. The observer exports only field geometry. The driver types only the generated local-test credentials from an explicitly supplied private file. It never reads a password from the game or a personal account. The server operator can arrange the fixed scene through ordinary server controls while retaining the player's ordinary role.

The optional fixture credential file contains `account` and `password` strings and must have private file permissions. This file stays outside Git. The account must match `--account` exactly. This works around the inspected game's bootstrap popup not restoring the newer Favorites account model.

```sh
proofs/performance/target/release/luabuddy-performance --game-dir /path/to/game --runtime target/release/libluabuddy_host.so --observer proofs/performance/target/release/libluabuddy_performance_proof.so --out /path/to/evidence/prime --mode world --single baseline --rounds 1 --warmup 30 --seconds 5 --deadline 900 --jvm-args /path/to/connect.json --auto-ui --account DisposablePerf --prime-cache-out /path/to/private-fixture
```

The first run requires normal UI input for Terms of Service, account creation, and character creation. The private fixture is copied only after the client closes normally. An unvalidated recovery has a different name and is not an accepted cache snapshot.

After the deciding release artifact is fixed and other verification builds are idle, run five alternating baseline and enabled arms. Each arm receives a separate copy of the same private fixture. Select a new evidence directory for every suite.

```sh
proofs/performance/target/release/luabuddy-performance --game-dir /path/to/game --runtime target/release/libluabuddy_host.so --observer proofs/performance/target/release/libluabuddy_performance_proof.so --out /path/to/evidence/deciding --template-cache /path/to/private-fixture --mode world --rounds 5 --warmup 30 --seconds 30 --deadline 300 --jvm-args /path/to/connect.json --auto-ui --account DisposablePerf --fixture-credentials /path/to/private-credentials.json --deciding
```

The runner writes the exact artifact hashes, raw frame intervals, process samples, native readiness, screenshots, and run summaries. It seeds the normal showSurvivalGuide=false preference in every disposable arm and can dismiss an unexpected visible Guide through its native Close button before the usable-world milestone and uses the native pause-menu Quit and confirmation after measurement. The final state must retain the ordinary player role and exact initial position. It owns and reaps each JVM and display. Successful runs require a normal zero exit and a fresh complete-log check after cleanup; cleanup failures cannot become accepted measurements. It refuses an evidence directory containing a previous workload.json, while allowing an empty or unused directory. It rejects unknown UI controls or a control from another process. Extra JVM arguments cannot replace its agents, user home, or empty LuaBuddy selection. The runtime path does not establish optimization; verify the artifact digest against the release reproducibility evidence before using it for acceptance.

The observer uses a VM-managed daemon agent thread to read existing native UI and world state. It requests no exclusive JVM capabilities and does not modify bytecode, fields, or methods. Mesa's hidden `frametime` HUD query records every renderer frame into a line-buffered file. Zero-period collection and boundary row counts select consecutive real frame intervals after warmup. Both arms use the identical query and readiness observer. No executable Java or Lua helper, global method tracing, or game-file modification is involved. Menu mode is available for a narrower workload; it does not establish in-world dispatcher overhead.

## Measure on the NVIDIA display with Steam

The optional `--frame-source mango_hud` backend uses the installed MangoHud `v0.8.4` OpenGL observer on an explicitly supplied local X11 display. The current hardware check requires the NVIDIA OpenGL library and MangoHud OpenGL hook in the exact owned JVM's mappings. It wraps the bundled JVM with `mangohud --dlsym`; the installed wrapper uses `exec`, so process ownership and process samples keep the same PID. This is a native JVM workload in the initialized Steam context. The ordinary Steam launcher journey remains a separate functional check.

Supply `--display` and `--xauthority` together. The runner does not own or stop that display, does not focus another application, and never refocuses after its window loses focus. It waits for initial focus, requires the focused X11 window to belong to its exact child before sending controls, and uses client-relative pointer coordinates. The native observer also checks focus before accepting readiness and during the measured workload. Focus loss rejects the run. A held button or key is released before reporting a mid-press focus loss. Screenshots contain only the owned window on this backend. Keep the display available for the run and stop other owned verification clients and builds first.

The existing-display path also requires the interactive session's explicit `DBUS_SESSION_BUS_ADDRESS`, installed `qdbus6`, and `timeout`. It queries the discovered `org.freedesktop.ScreenSaver.GetActive` method before setup and launch, before UI input, and during workload sampling. A locked desktop, unavailable service, timeout, or ambiguous response rejects the run and records its stage and reason in `desktop-checks.jsonl`. X11 focus alone cannot establish that KDE is unlocked. The runner does not unlock the session, change idle policy, or simulate user activity. Already held input is released before any later check rejects the action.

On KDE, after the user unlocks normally, the installed native `kde-inhibit` can wrap the bounded command with `--power --screenSaver`. Its inhibitor lasts only while that command runs. It does not replace the locked-session guard or authorize unlocking an already locked desktop. Preserve the normal configured idle policy. An example with the discovered tools is `kde-inhibit --power --screenSaver proofs/performance/target/release/luabuddy-performance ...`, with the full arguments below and the correct interactive session bus inherited. The desktop queries and evidence writes add common instrumentation work to both arms. Their recorded query durations do not establish zero observer overhead.

Prime a living ordinary player and a saved Favorites entry in a private cache through normal game controls. `--native-favorite` selects only the exact supplied account, checks the native saved-password flag, and clicks its existing Connect button. It neither reads the password from the game nor uses the older bootstrap popup. The Favorites entry supplies the owned server's connection settings. Use the same immutable cache snapshot for every arm. Do not supply the earlier bootstrap `connect.json` for this path.

```sh
proofs/performance/target/release/luabuddy-performance --game-dir /path/to/game --runtime /path/to/verified-release/runtime.so --observer proofs/performance/target/release/libluabuddy_performance_proof.so --out /path/to/new-evidence/deciding --template-cache /path/to/private-fixture --mode world --rounds 5 --warmup 30 --seconds 30 --deadline 300 --auto-ui --account DisposablePerf --deciding --frame-source mango_hud --display :1 --xauthority /path/to/session-xauthority --steam --native-favorite
```

Each arm gets its own transparent MangoHud configuration and output directory. The configuration enables per-frame CSV logging, disables uploads, and leaves frame limits and vsync to the game's existing settings. It uses `alpha=0` and `background_alpha=0` rather than `no_display`, because the inspected `v0.8.4` OpenGL path can skip the autostart code while the display is hidden. The driver records the installed wrapper and hook hashes. It requires successful native Steam initialization in the complete client log, Steam JNI/API mappings, and `SteamUtils.isSteamModeEnabled()` at both usable-world boundaries. It records no Steam identity value and invokes no account operation.

The CSV parser uses the actual header's `frametime` and `elapsed` columns. The matching [MangoHud logger](https://github.com/flightlessmango/MangoHud/blob/v0.8.4/src/logging.cpp) writes frame duration in milliseconds and elapsed time in integer nanoseconds and flushes each row. Its [OpenGL update path](https://github.com/flightlessmango/MangoHud/blob/v0.8.4/src/overlay.cpp) logs each presentation when `log_interval=0`. Its [GLX hook](https://github.com/flightlessmango/MangoHud/blob/v0.8.4/src/gl/inject_glx.cpp) observes before the real swap. These are application presentation-call intervals, not monitor scanout or input latency.

Before the pilot, the cadence checks require exactly one raw CSV, unambiguous frame headers, complete newline-terminated rows, matching field counts, finite nonnegative durations, and strictly increasing elapsed clocks. Boundary row counts select the same consecutive rows for timing and percentile calculations. The selected duration sum must agree with the elapsed span from the preceding boundary row within the larger of 100 ms or two percent of that span. The CSV span must agree with the observer's monotonic measurement window within 500 ms plus its largest selected frame. These tolerances account for row-boundary sampling and clock placement; they do not change the product's performance budget. Raw row indices, both clocks, sums, tolerances, and the CSV path remain in the report. Confirm the real pilot renders normally and produces per-frame cadence before running the deciding suite. Source and runtime-hook evidence support the logging rate; the cadence check alone cannot prove every swap was recorded.

The [fixed acceptance budget](../../docs/evolution/performance-budget.md) governs the result. A failed, incomplete, or noisy suite does not become a pass because an archive or report exists.

Run lifecycle growth as a separate workload after the paired comparison. The Rust driver creates two disposable test-content profiles. One has a cooperative cleanup contract and one native OnTick callback. The second supplies an independent foreign callback. These small Lua fixtures are test content executed by the host, like the existing runtime test fixtures. They are not product helpers or release contents. The verifier and scenario driver remain Rust.

```sh
proofs/performance/target/release/luabuddy-performance --game-dir /path/to/game --runtime target/release/libluabuddy_host.so --observer proofs/performance/target/release/libluabuddy_performance_proof.so --out /path/to/evidence/growth --template-cache /path/to/private-fixture --mode world --single enabled --rounds 1 --warmup 30 --seconds 30 --deadline 1800 --jvm-args /path/to/connect.json --auto-ui --account DisposablePerf --fixture-credentials /path/to/private-credentials.json --growth
```

The fixed lifecycle plan performs 11 no-op refreshes, 11 cooperative reloads, and 6 reconnects in one JVM. It discards the first cycle of each kind, leaving 10, 10, and 5 measured cycles. Native Options buttons perform refresh and reload on the game owner thread. Native leave-world and Favorites account controls perform reconnects. Each operation must finish with successful diagnostics, the expected generation/session contract, and an ordinary alive player at the original position. Generation IDs come from a shared allocator, so replacements and sessions may skip numbers. Both real fixture callbacks must advance beyond a baseline captured after the operation completes and the world resumes. Idempotent navigation is retried only while its native control remains observed visible/enabled; actual Refresh/Reload requests remain one-shot per cycle.

After each operation, the verifier drops temporary JNI locals, requests one `System.gc()` on its next poll, waits five seconds, and samples RSS every 250 ms for two seconds. It records JVM heap used/committed bytes, exact native OnTick callback counts, and callback/namespace identities. Reload weak probes cover the replaced owned callback and its exported LuaBuddyGrowth table. Reconnect adds the old LuaBuddy namespace and foreign callback. Identity uses temporary local upgrades of weak references, cleared before the later GC poll. The foreign module table, every JNI registration, and all native allocations are outside that weak-probe scope. RSS alone cannot establish retained objects, and JVM or renderer caches may retain committed pages. An incomplete scenario or live retired weak references are reported separately from a bounded-reference result.

To investigate reconnect growth without the product, use the explicit native-only diagnostic mode with the same private fixture and server. The native Favorites account must have its generated local-test password saved; the verifier checks that flag without reading the password.

```sh
proofs/performance/target/release/luabuddy-performance --game-dir /path/to/game --runtime target/release/libluabuddy_host.so --observer proofs/performance/target/release/libluabuddy_performance_proof.so --out /path/to/evidence/native-control --template-cache /path/to/private-fixture --mode world --single baseline --rounds 1 --warmup 30 --seconds 30 --deadline 1800 --jvm-args /path/to/connect.json --auto-ui --account DisposablePerf --fixture-credentials /path/to/private-credentials.json --native-reconnect-control
```

This mode creates no test profiles and loads no LuaBuddy agent. It requires the native menu transition, a different native Lua environment, resumed ordinary fixed-player state with zero namespace/profiles/native mods, and completed Mesa rows. It runs six reconnects, discards the first, and uses the same single-GC, five-second settle, and two-second RSS window. Its weak probes cover the retired native Lua environments. The control has no preceding Refresh/Reload history, so it establishes whether growth also happens without LuaBuddy; non-interleaved trends do not establish a precise incremental loader cost. Preserve an exact-owned-process map observation of observer presence and product absence beside the data.

The accepted measurements, diagnostic control, exact helper identities, and unresolved acceptance gates are recorded in the [performance evidence report](../../docs/evolution/performance.md). Raw evidence stays outside Git. Dispose of the private account cache and credential file after verification, then re-prime a fresh owned fixture for a future rerun.
