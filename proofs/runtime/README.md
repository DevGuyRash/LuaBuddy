# Verify the production runtime

Build the workspace, then run this Rust consumer against an installed Linux game and its bundled JVM. The second argument optionally selects a different built LuaBuddy agent library.

```sh
cargo build --workspace --locked --offline
JAVA_HOME=/path/to/ProjectZomboid/projectzomboid/jre64 cargo run --locked --offline --manifest-path proofs/runtime/Cargo.toml -- /path/to/ProjectZomboid/projectzomboid
```

Set `LUABUDDY_PROOF_EVIDENCE` to a directory outside the repository to retain selected actual diagnostic reports. The consumer copies the selected native artifact into its owned fixture directory and prints the SHA256 of those exact immutable bytes before loading them.

The consumer starts a separate JVM with the production native agent, `-Xcheck:jni`, and `-Xverify:all`. It initializes the actual game classes, gives them a disposable home and cache, and creates synthetic third-party mod fixtures in its temporary directory. Those fixture Lua files and assertion snippets are test inputs. They are never included in the production native library or used as application implementation.

The proof enters the production loader through real `LuaEventManager.triggerEvent` dispatch and invokes the actual `LuaBuddy.refresh` and `LuaBuddy.reload` callables. It checks actual Lua objects, callback behavior, on-disk profiles, and diagnostic reports. It does not import a duplicate copy of the production runtime into the consumer.

The scenarios cover dependency phases, common/version overlays, shadowed source execution, import precedence, nil/false/object/closure identity, late imports, nested source metadata, native delegation, no-op refresh, runtime addition, scoped event replay, reconnect, retired callables, compile/execution/circular-import failures, unrelated continuation, cooperative cleanup, cleanup refusal, compilation before cleanup, quarantine, callback ownership, dependency-based reload refusal, lifecycle/thread/reentrancy guards, and custom-hook refusal. It also checks that failed session records remain visible through later refreshes, rejected candidates do not replace loaded-generation diagnostics, and dedicated servers keep their native environment.

The consumer removes its owned temporary fixture directory when it exits. It does not launch a graphical client, contact a game server, change Workshop subscriptions, or touch the player's configuration. Graphical controls, actual third-party profile behavior, multiplayer authority, Workshop account operations, performance, and other operating systems need separate checks.
