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
proofs/performance/target/release/luabuddy-performance --game-dir /path/to/game --runtime target/release/libluabuddy_host.so --observer proofs/performance/target/release/libluabuddy_performance_proof.so --out /path/to/evidence/prime --mode world --single baseline --rounds 1 --warmup 30 --seconds 5 --deadline 900 --auto-ui --account DisposablePerf --prime-cache-out /path/to/private-fixture
```

The first run requires normal UI input for Terms of Service, account creation, and character creation. The private fixture is copied only after the client closes normally. An unvalidated recovery has a different name and is not an accepted cache snapshot.

After the deciding release artifact is fixed and other verification builds are idle, run five alternating baseline and enabled arms. Each arm receives a separate copy of the same private fixture. Select a new evidence directory for every suite.

```sh
proofs/performance/target/release/luabuddy-performance --game-dir /path/to/game --runtime target/release/libluabuddy_host.so --observer proofs/performance/target/release/libluabuddy_performance_proof.so --out /path/to/evidence/deciding --template-cache /path/to/private-fixture --mode world --rounds 5 --warmup 30 --seconds 30 --deadline 300 --jvm-args /path/to/connect.json --auto-ui --account DisposablePerf --fixture-credentials /path/to/private-credentials.json --deciding
```

The runner writes the exact artifact hashes, raw frame intervals, process samples, native readiness, screenshots, and run summaries. It seeds the normal showSurvivalGuide=false preference in every disposable arm and can dismiss an unexpected visible Guide through its native Close button before the usable-world milestone and uses the native pause-menu Quit and confirmation after measurement. The final state must retain the ordinary player role and exact initial position. It owns and reaps each JVM and display. Successful runs require a normal zero exit and a fresh complete-log check after cleanup; cleanup failures cannot become accepted measurements. It refuses an existing evidence directory and rejects unknown UI controls or a control from another process. Extra JVM arguments cannot replace its agents, user home, or empty LuaBuddy selection. A release runtime path is an operator assertion; verify that artifact against the release reproducibility evidence before using it for acceptance.

The observer uses a VM-managed daemon agent thread to read existing native UI and world state. It requests no exclusive JVM capabilities and does not modify bytecode, fields, or methods. Mesa's hidden `frametime` HUD query records every renderer frame into a line-buffered file. Zero-period collection and boundary row counts select consecutive real frame intervals after warmup. Both arms use the identical query and readiness observer. No executable Java or Lua helper, global method tracing, or game-file modification is involved. Menu mode is available for a narrower workload; it does not establish in-world dispatcher overhead.

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
