# Rust evolution status

The original user request is preserved in [request.md](request.md). This document records implementation and acceptance status. It does not change the requested outcome.

The migration starts at commit `84d495a`. Its baseline `python3 tests/check.py --offline` passed during this run against Project Zomboid `42.22.2 3d39148846 integrity=pending`, bundled Java `25.0.5`, and the existing verified ZombieBuddy `2.3.4` cache. That result verifies the old implementation only.

## Workflow

1. Read the poteto Principles and frame the acceptance gates from the original request.
2. Establish native callable/dispatch, Workshop bootstrap, and the complete existing behavior inventory independently.
3. Prove the Rust callback mechanism in the actual bundled JVM before depending on it.
4. Implement host-independent policy, patch registry and extension ABI, reversible installation, the game adapter, all bundled hooks, and the in-game interface.
5. Build the complete change before writing new tests. Exercise the real user path, then cover remaining integration and policy gaps.
6. Obtain independent GPT review, resolve findings, rerun final checks, package with integrity manifests, and deliver through the repository.

## Acceptance gates

| Gate | Status | Evidence |
| --- | --- | --- |
| Existing implementation baseline | Passed | Aggregate offline check during this run. |
| Rust callback through actual host callable | Passed | Exact values/identity, nested calls, exceptions, real dispatcher/queue, actual UIElement and native notifications passed under JNI/verifier checks. Full game UI and multiplayer checks also passed. |
| Ordinary Linux launcher override | Passed | The copied real launcher covered clean install, native menu and callback, offline relaunch, update, rollback, uninstall, and preserved settings. The later normal Steam launch mapped runtime `17ffaedb`; fresh diagnostics and `doctor` confirmed all 24 existing selections loaded with no failed, pending, or restart-required profiles. |
| Workshop subscribe-only native activation | Unresolved | Inspected mod discovery and mounting deliver files without a native activation action. No clean subscribe/launch/update/offline/removal proof. |
| Rust behavior migration | Passed for tested scope | All 24 profiles and 16 hooks ported; exact legacy migration, 19 actual-runtime scenarios, 24 isolated world cases, all-24 combination and no-op/reconnect passed. |
| In-game management | Passed for tested Linux scope | Actual installed/subscribed/Workshop search, version/dependency details, selection, native startup/relaunch, live addition and diagnostics passed. A later authorized Steam test completed a fresh subscription/download on candidate `17ffaedb` and exercised that mod's actual overlays, saved options, repeated no-op refresh, and reconnect. |
| General-purpose patcher and two extensions | Passed | Several unrelated JVM targets, compiled extensions, normal production approval/activation, conflicts, nesting/concurrency and scoped disable passed. |
| Release and review | Reproducible Linux package and independent reviews passed; acceptance remains open | Exact optimized package `1d44f599` passed 44 aggregate checks after the test-only portability correction. [Draft PR #17](https://github.com/DevGuyRash/LuaBuddy/pull/17) tracks portable CI for its current head. The earlier software-rendered five-pair comparison is inconclusive. The later NVIDIA comparison stopped after four of ten required runs, so it establishes no speedup or budget pass. The full 28-cycle lifecycle run collected all scoped retired references; reconnect RSS crossed the investigation threshold, and a native-only control also reproduced substantial growth. Incremental loader cost, physical controller behavior, and other OS game execution remain unverified. |

The user explicitly instructed `no claude at all this time`. Implementation and review therefore use GPT models, overriding the repository's model-family requirement for this task.

The November 10 follow-up completed the authorized real Steam delivery and gameplay test and restored the test item's original subscription and installed state. Review also found and fixed profile directory metadata disagreeing with the launcher after rollback. The candidate passed 82 workspace tests, 24 performance-tool tests, the 44-check native offline aggregate, and two-path reproducibility. These Steam and normal-game observations use runtime `17ffaedb`; later builds do not replace that evidence identity. The Steam and rollback fixes have signed source commits; PR #17 records the delivery checks for its current head.

The user then requested normal Steam activation. The installer preserved all 24 existing selections, migrated 16 recognized legacy hook profiles, retained the original Lua files and recovery copies, and left the base launcher JSON and executable unchanged. A normal Steam process mapped the installed Rust runtime. Its fresh startup report completed with 24 loaded profiles and zero failures, pending profiles, or restart requirements, and `doctor` passed. The running game was observed without sending it input or changing its state.

Portable CI then found that `DownloadRequester::Test` existed in Windows and macOS test builds even though its constructors are Linux x86_64 only. The variant and match arm now share that platform condition. After the correction, the whole workspace build, strict Clippy, and 82 behavior tests passed. Fresh direct-toolchain builds reproduced runtime `1d44f599`, CLI `e4aa550b`, all 32 package files, and the normalized archive. The new exact package passed 44 native checks with four separately identified workflows not run. Final current-head CI remains tracked in PR #17.

The latest package is installed for the next normal Steam launch. The update preserved all 40 current profile-file hashes and passed verification and `doctor`. The running game continues with the already verified `17ffaedb` runtime and its completed 24-profile startup. It was not forced to restart. No new graphical or performance journey is claimed for `1d44f599`.

The latest NVIDIA attempt completed two alternating pairs and 7,107 verified presentation intervals before desktop focus interrupted the fifth run. The user then said, "I'm going to play zomboid. lets work off what we have and did find". Six required runs were not collected. The partial data remains separate from earlier attempts and establishes no performance acceptance. Owned verification clients, displays, and the private server stopped; disposable caches and generated fixture credentials were removed. The server required bounded termination after native quit completed its save. Evidence remains outside Git, and no verification automation remains running.
