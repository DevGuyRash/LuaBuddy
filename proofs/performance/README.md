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
proofs/performance/target/release/luabuddy-performance --game-dir /path/to/game --runtime target/release/libluabuddy_host.so --observer proofs/performance/target/release/libluabuddy_performance_proof.so --out /path/to/evidence/deciding --template-cache /path/to/private-fixture --mode world --rounds 5 --warmup 10 --seconds 30 --deadline 300 --jvm-args /path/to/connect.json --auto-ui --account DisposablePerf --fixture-credentials /path/to/private-credentials.json --deciding
```

The runner writes the exact artifact hashes, raw frame intervals, process samples, native readiness, screenshots, and run summaries. It dismisses the observed native Survival Guide before the usable-world milestone and uses the native pause-menu Quit and confirmation after measurement. It owns and reaps each JVM and display. It refuses an existing evidence directory and rejects unknown UI controls or a control from another process. Extra JVM arguments cannot replace its agents, user home, or empty LuaBuddy selection.

The observer uses a VM-managed daemon agent thread to read existing native UI and world state. It requests no exclusive JVM capabilities and does not modify bytecode, fields, or methods. Mesa's hidden `frametime` HUD query records every renderer frame into a line-buffered file. Zero-period collection and boundary row counts select consecutive real frame intervals after warmup. Both arms use the identical query and readiness observer. No executable Java or Lua helper, global method tracing, or game-file modification is involved. Menu mode is available for a narrower workload; it does not establish in-world dispatcher overhead.

The [fixed acceptance budget](../../docs/evolution/performance-budget.md) governs the result. A failed, incomplete, or noisy suite does not become a pass because an archive or report exists. Reload and reconnect growth require their separate lifecycle scenario.
