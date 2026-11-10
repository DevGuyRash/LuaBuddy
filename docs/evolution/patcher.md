# Rust patch engine and extension ABI

`luabuddy-patcher` owns patch policy, exact target matching, installation state, Rust callback dispatch, and native-extension approval. The JVM adapter owns capabilities, classloader resolution, bytecode inspection, breakpoints, local-variable access, and early returns. A registered patch does not become installed until that adapter returns an installation receipt.

## Patch registry

`PatchSpec` declares a stable `id`, `owner`, exact `Target`, `kind`, `requires`, signed `priority`, and `reentrancy`. `Target` contains an internal JVM class name, method name, complete JVM descriptor, adapter-defined classloader identity, and the SHA256 of the inspected method bytecode. A stale signature, loader, or fingerprint fails before installation. An empty default registry installs nothing.

`Registry::register` validates metadata and rejects duplicate IDs. `Registry::plan` includes required patches, rejects missing dependencies and cycles, orders ready patches by ascending priority and then stable ID, and rejects competing replacement, argument-edit, or result-edit owners for the same target. Different fingerprints for one method also conflict. Registration and planning never modify the host.

`Registry::activate` inspects every new target before installing any patch. Multi-patch activation requires a reversible backend. A failed installation rolls back this operation's receipts in reverse order. An uncertain rollback quarantines the affected patch and blocks new activation until removal succeeds or the process restarts. Previously installed patches remain installed. The backend contract requires each individual failed install to leave the host unchanged.

`Registry::disable` refuses removal while an installed dependent needs the patch. It calls the backend before changing the installed state and is idempotent after success. An uninstall error retains the live receipt. Disabling a quarantined receipt still requires a successful backend uninstall.

Diagnostics distinguish `registered`, `installed`, `rejected`, `failed`, `disabled`, and `quarantined`. A successful preflight is never reported as JVM installation.

## Capability contract

`PatchBackend::capabilities` starts empty. The JVM adapter advertises only capabilities it has verified. The current candidate selective-breakpoint mechanism can potentially support entry observation, local argument edits, and typed early return. The native proof determines which of these are actually enabled.

| Hook kind | Dispatcher behavior | Native requirement |
| --- | --- | --- |
| `before` | Observes entry without frame mutation. | Verified entry callback. |
| `replace` | Returns `Continue` or a typed `Return`. A returned value cancels the original call. | Verified typed early return. |
| `modify_arguments` | Edits typed arguments at entry. | Verified local-variable writes. |
| `after_normal_return` | Observes normal completion. | Verified completed-return observation. |
| `modify_result` | Edits the typed normal result. | Verified access to and replacement of that result. |
| `after_exceptional_return` | Observes an exceptional completion without swallowing its exception. | Verified exceptional completion observation. |
| `around` | Rejected unconditionally. | No proven dispatcher exists. |

A breakpoint at a return instruction does not itself prove access to the JVM operand stack. The registry's ability to dispatch a result hook through a test backend does not establish that a JVM adapter supports that hook. Unsupported capabilities produce an explicit error.

`Invocation` carries a target, stage, typed arguments, optional typed result, and optional exception diagnostic. Arguments exclude the receiver. Object values are host-owned opaque tokens, never raw JNI addresses. The adapter establishes token lifetimes and resolves the receiver separately. Primitive types follow JVM descriptors, including `Int` for boolean, byte, char, short, and int.

Entry callbacks follow dependency order and priority. Normal-return callbacks run in reverse order. Only a replacement hook can return early. Only the corresponding mutation hook can edit arguments or a result. The registry validates types and restores the frame supplied to a failing hook. It cannot undo external side effects performed by a callback.

`SkipSelf` suppresses the same patch during a nested call on the same thread. `Reject` reports a reentrancy error. Other patches can still observe nested calls. Different threads keep independent nesting state. Rust hooks must be `Send + Sync`. Panics inside Rust hooks become errors before returning to the adapter. The adapter must also catch panics at every JNI or JVMTI entrypoint.

## Native-extension approval

`ExtensionManifest` is a fixed JSON schema with unknown fields rejected. It records schema and ABI versions, extension identity, artifact SHA256, provenance, process OS and architecture, declared privileges, and an ordered list of patch declarations. The library supplies callbacks in exactly that list order. Every patch must be owned by the extension.

`EnableConsent` binds explicit install and enable intent to the same extension, exact artifact SHA256, provenance, and permitted privileges. `extension::verify` checks these conditions and the actual artifact bytes before any library is loaded. Merely downloading Workshop content never creates approval.

Native code has the process's OS privileges. Declared patch privileges validate intent and patch policy. They do not sandbox a library's filesystem, network, or memory access. Provenance is an explicitly approved source identity, not a cryptographic author signature.

`LoadedExtension::load` is unsafe because the caller must establish that the approved binary and its native dependencies obey the ABI and remain unmodified during loading. Loading can execute native initializers. An artifact hash alone cannot establish memory safety or remove filesystem races against another process with the same write permissions. The installer must use controlled owned files, and the operator must trust the approved code. Once loaded, the library stays mapped until process exit, including after a descriptor rejection or patch disable. Native unloading is unsupported because background extension work cannot be proven quiescent.

## ABI v1

The exported symbol is `luabuddy_extension_v1`. It has the C calling convention and returns `ExtensionV1` by value. `repr(C)` structures contain fixed-width integers, fixed arrays, and optional function pointers. No Rust allocator, string, collection, trait object, or exception crosses the boundary.

`ExtensionV1` contains ABI version 1, its exact structure size, a zero-terminated UTF-8 identity in 64 bytes, a patch count, a zero reserved field, and 32 callback slots. Slots beyond the declared count must be null. Version, size, identity, count, and reserved fields must match before callbacks are registered.

Each callback receives exclusive, aligned host-owned `FrameV1` storage valid only for that call. The callback cannot retain the pointer. It must accept concurrent calls on distinct frames and nested calls, and it must never unwind. The frame carries a stage, up to 32 arguments, and an optional result. The immutable version, size, stage, argument count, and reserved fields must survive the call unchanged.

`ValueV1.tag` is 0 for void, 1 for a JVM int-like value, 2 for long, 3 for float, 4 for double, and 5 for an opaque object token. `bits` contains the two's-complement integer representation, IEEE floating-point bits, or token. Reserved fields are zero. Extensions can return only an object token supplied in that same frame or token zero for null.

A callback returns 0 to continue, 1 to replace the call using its result, or another status to fail. The patch kind still governs which edits are permitted. A replacement result must match the method's declared return type.

## Build and exercise the Rust examples

The two independent examples are `math-guard`, which computes a saturating integer absolute value, and `string-observer`, which counts observed calls atomically. Neither example is installed or enabled by default. The probe supplies declarative fixture targets and an explicit contract backend. It exercises binary loading and Rust callback behavior without claiming to patch the live game or JVM.

```sh
cargo build --manifest-path examples/extensions/math-guard/Cargo.toml --locked --offline --target-dir target/extensions
cargo build --manifest-path examples/extensions/string-observer/Cargo.toml --locked --offline --target-dir target/extensions-observer
cargo run -p luabuddy-patcher --example extension_probe --locked --offline -- target/extensions/debug/libluabuddy_math_guard.so target/extensions-observer/debug/libluabuddy_string_observer.so
cargo test -p luabuddy-patcher --locked --offline
```

These commands use Linux library filenames. Windows and macOS extension loading and game execution remain unverified until exercised on those systems. Building an extension requires the pinned build toolchain. Loading an already compiled approved extension does not invoke a compiler.
