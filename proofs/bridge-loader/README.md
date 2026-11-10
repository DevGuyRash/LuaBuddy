# Native bridge classloader isolation proof

This Rust proof prepares an untransformed copy of the actual game's `LuaEventManager` through a separate `URLClassLoader` while the supported system class has been transformed but has not reached `ClassPrepare`. The unsupported copy must retain its original Java declaration and no-op result. The supported copy must register its native declaration. This reproduces the registration interval deterministically without changing the game JAR, generating Java classes, or using the user's game session.

Set `PZ_JAVA` to the selected game's bundled Java executable and `PZ_JAR` to its installed game JAR. Build before running the proof. Run it with a disposable home directory.

```sh
cargo build --manifest-path proofs/bridge-loader/Cargo.toml --locked --offline --target-dir target/bridge-loader-proof
proof_home=$(mktemp -d)
timeout 30s "$PZ_JAVA" -Xcheck:jni -Xverify:all "-Duser.home=$proof_home" "-Dluabuddy.proof.jar=$PZ_JAR" "-agentpath:$PWD/target/bridge-loader-proof/debug/libluabuddy_bridge_loader_proof.so" -cp "$PZ_JAR" -version
proof_status=$?
rm -rf -- "$proof_home"
test "$proof_status" -eq 0
```

The agent exits with failure when either class fails to prepare, the unsupported declaration changes, or the supported native registration is missing. A successful run prints `[LuaBuddy bridge loader proof] PASS`. The bridge's unsupported-classloader rejection log is expected because that class is deliberately outside LuaBuddy's supported loader. JNI warnings, verifier failures, or any proof `FAIL` are failures.
