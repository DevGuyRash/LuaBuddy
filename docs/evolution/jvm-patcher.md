# JVM patch adapter

`luabuddy_host::patcher::JvmPatcher` connects the reusable Rust registry to actual JVMTI breakpoints. The agent initializes this adapter with the same JVMTI environment that owns its capabilities and callbacks. Calling JVM `GetEnv` again would create a separate JVMTI environment, so the adapter never reacquires one during activation.

## Implemented operations

| Operation | Native behavior | Limits |
| --- | --- | --- |
| Before | An entry breakpoint reads integer and reference arguments and dispatches Rust callbacks in registry order. | Constructors, native methods, abstract methods, and wide numeric arguments are rejected. |
| Integer argument mutation | Rust updates the typed invocation, then JVMTI `SetLocalInt` commits the final values before the host method executes. | Every argument must be integer-like. A failed write attempts to restore earlier writes and reports any uncertainty. |
| Integer replacement | JVMTI `ForceEarlyReturnInt` returns the Rust result instead of executing the original body. | Supports JVM boolean, byte, char, short, and int returns. Boolean replacements must be zero or one. |
| Disable | The adapter removes the exact receipt and clears the physical breakpoint when its last receipt is removed. | A callback that entered earlier may finish. Later entries use the current receipt set. |
| After, result mutation, around, and exceptional-return hooks | Activation returns an unsupported-operation error. | The adapter does not read operand-stack results or pretend that an entry breakpoint brackets exceptional completion. |

Target metadata specifies the exact internal class name, method name, descriptor, bootstrap or system classloader, and SHA256 of the host method bytecode. Fingerprints are checked again during installation. The adapter rejects inherited-method aliases, unsupported layouts, and a target already owned by another patcher instance. Declaring-class global references keep installed method identities alive.

Lookup requests `Class.forName` with initialization disabled and enumerates declared methods through JVMTI. It does not use JNI method-ID lookup to force target static initialization. If normal host lifecycle has not prepared a class yet, activation reports that condition and leaves it inactive. The proof exercises each target's original baseline before observing and activating its patch.

Several compatible patches in one registry share an entry breakpoint. Each has its own installation receipt. Registry dependencies and priority determine callback order. An immutable registry snapshot is filtered through the current live receipt IDs before dispatch, so disabling one shared hook does not run it through a stale snapshot. Locks protect short ownership updates and snapshot acquisition. JVM operations and Rust hook execution run after those locks are released.

Reentrancy keys include a unique logical registry identity and its local patch ID. Independent registries may use the same ID without suppressing one another. Clones retain the registry identity so immutable dispatcher snapshots still skip themselves during nested calls. Control operations remain with the owning registry.

Reference arguments use temporary integer tokens. Zero means null, and repeated references to the same object receive the same token within that invocation. Tokens are not memory addresses, cannot be retained for another call, and do not grant access to arbitrary JNI objects. Reference argument and reference result replacement are unsupported.

Rust hook errors and caught Rust panics leave the invocation unchanged and report a callback failure. A JNI exception left by a Rust hook is cleared before returning to the host dispatcher. The original host method then runs. Host exceptions raised by that original method retain normal JVM behavior. `callback_failures()` reports the last failure for the affected live target receipts. The adapter cannot undo arbitrary side effects performed by trusted extension code.

Keep the `JvmPatcher` handle for the lifetime of its active patches. Dropping it does not uninstall them. Call `disable` explicitly for each patch. Native extension libraries stay mapped until process exit, as documented in [the extension ABI](patcher.md).

## Executable proof

The proof uses a separate JVM, the selected game's installed JAR, and the two repository-built Rust extension libraries. It does not launch the game, read saves, modify a user cache, or change Workshop subscriptions. All proof behavior is Rust. It adds no Java or Lua implementation helper.

Build the complete native adapter before running the proof.

```sh
cargo build --manifest-path proofs/patcher/Cargo.toml --locked --offline --target-dir target/patcher-proof
cargo build --manifest-path examples/extensions/math-guard/Cargo.toml --locked --offline --target-dir target/extensions
cargo build --manifest-path examples/extensions/string-observer/Cargo.toml --locked --offline --target-dir target/extensions-observer
cargo run --manifest-path proofs/patcher/Cargo.toml --locked --offline --target-dir target/patcher-proof -- "$PZ_JAVA" "$PZ_JAR" target/patcher-proof/debug/libluabuddy_patcher_proof.so target/extensions/debug/libluabuddy_math_guard.so target/extensions-observer/debug/libluabuddy_string_observer.so
```

The Rust runner sets `-Xcheck:jni` and `-Xverify:all`, records the selected game JAR digest, requires the positive proof marker, rejects verifier/JNI warnings and failures, and kills its owned child if the 30-second deadline expires. Set `PZ_JAVA` and `PZ_JAR` to the selected installation before running this Linux command.

The November 9, 2026 run passed on Linux x86_64 with bundled Zulu Java `25.0.5+8-LTS`. The selected game JAR SHA256 was `3c4a7c5f149d784758c6fb5f8fa85f3a21d572e38bc081400e89035f8120d287`.

| Proof | Independent expected result |
| --- | --- |
| Compiled `math-guard` extension on `Math.abs(int)` | The Java baseline returns `Integer.MIN_VALUE` for its minimum argument; the Rust saturating replacement returns `Integer.MAX_VALUE`; disable restores the baseline. |
| Compiled `string-observer` extension on the game's `StringUtils.isNullOrEmpty(String)` | The empty string still returns true, and the extension's actual atomic counter increases exactly once. It stops increasing after disable. |
| Co-located hooks on `Integer.sum(int,int)` | Priority orders observation of 2, mutation to 7, and observation of 7. The host result is 10. Removing the mutator restores 5 while both observers remain, then removing either observer preserves the other. |
| Nested `Integer.compare(int,int)` | A callback's nested call to the same target skips itself, retains the native result, and increments its observation only once. |
| Independent nested registries on `Integer.max(int,int)` and `Integer.min(int,int)` | Both registries use local patch ID `shared`. The outer hook's inner method call still invokes the independent inner hook exactly once. |
| Concurrent `Integer.compare(int,int)` | Four attached JVM caller threads each receive the original result and produce exactly one observation. |
| Concurrent disable on `Integer.signum(int)` | A deliberately blocked entered callback can finish after disable. A later entry proceeds without that callback, and both host results remain correct. |
| Exceptional `Integer.parseInt(String)` | An invalid number still raises the original `NumberFormatException` after exactly one before-hook observation. |
| Reference identity on `Arrays.equals(int[],int[])` | A repeated array receives equal transient tokens; different equal arrays receive different tokens; native equality remains true. |
| Compatibility and ownership refusal | A stale fingerprint, incorrect classloader, unsupported wide argument layout, unsupported after hook, and an overlapping patcher owner all fail before installing their requested patch. |

This proof establishes the listed native mechanisms and targets. It does not establish acceptable game frame-time overhead, Windows or macOS execution, arbitrary bytecode transformation, or every third-party patch combination. Those remain separate release gates.
