# Native event dispatch

The November 13, 2026 comparison found lower headless event-dispatch cost after replacing the lifecycle breakpoints with the native bridge in `0.1.0-rc.3`. This result covers the event path on Linux x86_64, Project Zomboid 42.22.2, and its bundled Azul Java 25.0.5+8-LTS. It does not establish an FPS gain, resolution of the reported gameplay hitches, or a pass of the full performance budget.

The old adapter installed entry and normal-return breakpoints on every `Event.trigger` call. Each callback obtained the receiver through JVMTI `GetLocalObject` before filtering event names. The new adapter uses the existing initial-class-definition and JNI registration mechanism. It retains the fingerprint-checked original body as a private method, calls it exactly once, and observes entry and successful return around it. It preserves the original boolean return and pending game exception. Other patch-engine and Workshop notification breakpoints retain their existing behavior.

## Matched release comparison

Five alternating baseline/candidate pairs each warmed every case with 10000 invocations and measured 100000 more. The baseline comes from `9d7a6a5d2303365dea0dd71f2fe97c81d8c12e5d`. All three million measured dispatches completed, with two million expected Lua callback invocations and zero measurement failures. These headless fixtures used disposable homes, caches, and empty profile selections. The user's game remained running. Observed machine load during the comparison was approximately 11 to 18 on 24 logical CPUs, so the arms alternated under that background work. Setup, compilation, and JFR recording were excluded from deciding timings.

| Workload | Baseline median and range, µs | Candidate median and range, µs |
| --- | --- | --- |
| `direct-empty` | 33.92 (32.68 to 35.18) | 1.47 (1.44 to 1.53) |
| `LuaBuddyMeasureCallback` | 45.52 (41.79 to 46.02) | 2.44 (2.33 to 2.55) |
| `OnTick` | 44.89 (42.04 to 47.30) | 2.09 (1.97 to 2.36) |

The empty case invokes the real dispatcher directly and checks every false return. The public game manager skips empty events, so an empty public trigger would not measure the changed path. The other cases call the public Lua event API with a listener, count completed iterations, and require exactly one callback per event. The tick case initializes a LuaBuddy menu session and has no pending favorite restoration. The remaining cost includes the JNI transition, original dispatcher, Lua loop, and callbacks. The benchmark does not compare game rendering or third-party mod work.

## VM operation attribution

Separate completed JFR runs used 1000 measured calls per case plus the same warmup and initialization. The baseline recorded 66002 `GetOrSetLocal` VM operations, matching two local-frame reads per dispatcher call. The candidate recorded zero. Both recordings covered the full fixture. Profiling results support removal of this safepoint source and remain excluded from the timing table. Early recordings that did not capture the complete fixture and a timed-out profiling pilot were excluded.

## Artifact identity and verification

The deciding baseline runtime SHA256 is `bfc3b5fac8cb393cd05d15f4fbcc90cd106931fbd4482b49e48619cfb62ed771`. The deciding candidate SHA256 is `bf6c7cb1d9adc696696f20f32c735f679628b2bea2fdfd595df5a6d26e3aeedd`. Raw samples, profiler recordings, and local host details remain outside Git under the LuaBuddy verification state root, in `2026-11-13-event-bridge`.

Native proofs cover unchanged original bytecodes, empty and populated boolean results, Unicode names including supplementary characters and NUL, exceptional exit without a normal-return notification, synchronous nested events, worker-queued dispatch, startup readiness, reconnect, and runtime profile lifecycles. The classloader proof checks that unsupported loader copies remain untransformed. The aggregate verification result and delivered package identities accompany the release provenance.

The bridge applies to the initial supported system-loader definition. Installing an update changes the next normal game launch. A running process keeps its mapped runtime. See [verification commands](../verification.md#measure-the-final-runtime) and the unchanged [full-game budget](performance-budget.md).
