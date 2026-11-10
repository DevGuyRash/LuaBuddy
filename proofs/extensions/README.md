# Verify normal native extension delivery

This Rust consumer exercises the normal LuaBuddy agent and CLI approval workflow. It does not select an agent proof mode or import another copy of the runtime manager. The selected game and bundled JVM remain external dependencies.

Build the application, independent Rust extensions, and the constructor observation fixture from the repository root.

```sh
cargo build --locked --offline
cargo build --locked --offline --manifest-path examples/extensions/math-guard/Cargo.toml --target-dir target/extensions
cargo build --locked --offline --manifest-path examples/extensions/string-observer/Cargo.toml --target-dir target/extensions-observer
cargo build --locked --offline --manifest-path proofs/extensions/marker/Cargo.toml --target-dir target/extensions-marker
JAVA_HOME="$PZ_GAME_DIR/jre64" cargo run --locked --offline --manifest-path proofs/extensions/Cargo.toml --target-dir target/extensions-product-proof -- "$PZ_GAME_DIR" "$PWD"
```

`PZ_GAME_DIR` names the actual directory containing the installed `projectzomboid.jar`. The proof snapshots the application executable, native runtime, and extension binaries once before its matrix. Each case receives a fresh JVM with JNI checking, bytecode verification, a private cache, and a private profile directory. The parent owns the entire scratch tree and enforces a 45-second child deadline.

Set `LUABUDDY_PROOF_RUNTIME` and `LUABUDDY_PROOF_CLI` to select immutable release snapshots instead of the default debug artifacts. The consumer records the copied CLI and runtime digests and uses that same bundle in every case. The extension example and marker paths remain the build destinations shown above.

| Case | Independent expected observation |
| --- | --- |
| Installed but disabled | No installed extension patches and no native constructor marker. Later disk approval changes do not become active through a running-process retry. |
| Unapproved content | A library without an approval is rejected. Its initializer does not run. |
| Changed artifact | The binary digest mismatch is rejected before loading, while independent approved extensions still install. |
| Changed manifest | A manifest digest mismatch is rejected before loading. Healthy approved extensions remain usable. |
| Metadata update with unchanged binary | A new immutable bundle and disabled approval are created. The prior exact-manifest approval cannot enable it. Prior bundle bytes remain unchanged and its initializer stays inactive. |
| Approved activation | The real JVM `Math.abs(Integer.MIN_VALUE)` changes to `Integer.MAX_VALUE`, the independent compiled observer counts an actual `StringUtils.isNullOrEmpty` call, and the approved marker initializer runs. |
| Deferred target | An unprepared `FrameDelay.update` target prevents native loading. Normal host construction prepares the class without automatic activation. The real `LuaBuddy.retryPatches()` callable then installs the original approved generation and publishes an actual installed receipt. |

The marker library writes only the explicitly provided disposable observation file. Its source is Rust and its initialization entry is an ELF constructor for this Linux x86_64 proof. It is verification content and is not included in application packages.

The November 9, 2026 run used Linux x86_64, Project Zomboid `42.22.2`, and bundled Java `25.0.5`. This proof does not establish full graphical gameplay, native unload safety, acceptable frame-time overhead, Workshop activation, or other operating systems. It performs no account mutation and leaves the user's running game untouched.
