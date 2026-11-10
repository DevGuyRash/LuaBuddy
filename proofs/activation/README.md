# Verify native startup selection

Build the production workspace before running this consumer. It imports the same Rust activation adapter and calls the actual installed game's metadata, preference serializer, and preference parser. It does not launch a game window, use Steam, or modify the user's cache.

Set `JAVA_HOME` to the selected game's bundled runtime. Pass the actual game directory containing `projectzomboid.jar`.

```sh
JAVA_HOME=/path/to/ProjectZomboid/jre64 cargo run --manifest-path proofs/activation/Cargo.toml --locked --offline -- /path/to/ProjectZomboid
```

The consumer enables `-Xcheck:jni` and `-Xverify:all`. A successful run ends with `PASS native startup selection and centralized live eligibility`. It cleans its disposable fixture directory on exit. The runtime and native game state assertions run in one fresh JVM, and no account API is called.

The fixture creates native metadata for a consumer, dependency, incompatible version, missing dependency, dependency cycle, and Unicode identity. Expected preference contents are specified independently in the consumer. Tests verify actual native file contents, original default selections, map order, current-game state, loaded-state comparisons, and the native active-ID list.

Linux native execution is verified. Other operating systems require their own game runtime checks before support is claimed.
