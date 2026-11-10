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
| Ordinary Linux launcher override | Passed | The copied real native launcher selected the installed `.site.json`. Eight stages covered clean install, actual menu and Rust callback, offline relaunch, update, rollback, uninstall, preserved settings, and repeated uninstall. |
| Workshop subscribe-only native activation | Unresolved | Inspected mod discovery and mounting deliver files without a native activation action. No clean subscribe/launch/update/offline/removal proof. |
| Rust behavior migration | Passed for tested scope | All 24 profiles and 16 hooks ported; exact legacy migration, 19 actual-runtime scenarios, 24 isolated world cases, all-24 combination and no-op/reconnect passed. |
| In-game management | Reads/selections/loading passed; writes unrun | Actual installed/subscribed/Workshop search, version/dependency details, selection, native startup/relaunch, live addition and diagnostics passed. User restricted live Workshop verification to reads. |
| General-purpose patcher and two extensions | Passed | Several unrelated JVM targets, compiled extensions, normal production approval/activation, conflicts, nesting/concurrency and scoped disable passed. |
| Release and review | Reproducible Linux package and independent reviews passed; acceptance remains open | Exact optimized package passed 44 aggregate checks. The signed implementation is pushed in draft PR #17 and portable CI passed on Linux, Windows, and macOS. The fixed five-pair comparison is inconclusive. The full 28-cycle lifecycle run collected all scoped retired references; reconnect RSS crossed the investigation threshold, and a native-only control also reproduced substantial growth. Incremental loader cost remains unestablished. Physical controller and other OS game execution remain unverified. |

The user explicitly instructed `no claude at all this time`. Implementation and review therefore use GPT models, overriding the repository's model-family requirement for this task.
