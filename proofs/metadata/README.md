# Verify native metadata and diagnostics

Run this consumer against an installed Linux game and its bundled JVM from the repository root.

```sh
JAVA_HOME=/path/to/ProjectZomboid/projectzomboid/jre64 cargo run --locked --offline --manifest-path proofs/metadata/Cargo.toml -- /path/to/ProjectZomboid/projectzomboid
```

The consumer creates a disposable home directory, cache, mod metadata, and translation files. It initializes actual game classes in a separate JVM with JNI checking and bytecode verification. It deletes its fixture directory on exit. It does not start a game client, alter Workshop subscriptions, or use the player's cache.

The metadata checks cover identity, required dependencies, version rejection, native registered symlink precedence, flat Workshop roots, unavailable Steam, and recovery after failed discovery. The translation checks use the native parser and formatter for common and version precedence, English, German, fallback, and placeholders. Callback checks use compiled host content and native event registration and removal. They check filename ownership, object identity, foreign callback preservation, and refusal when native removal does not take effect. Diagnostics checks read the actual generated report and compare session identity, running records, operation errors, timestamps, game version, and JVM version.

The checks passed on Project Zomboid `42.22.2 3d39148846 integrity=pending` with bundled Azul Java `25.0.5` on Linux x86_64 during the Rust migration. This consumer does not establish graphical interface behavior, live Workshop account operations, multiplayer loading, or Windows and macOS execution.
