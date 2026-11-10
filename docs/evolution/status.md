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
| Rust callback through actual host callable | Partial proof passed | Bundled JVM, `-Xcheck:jni`, `-Xverify:all`, exact argument identity on three calls. Full lifecycle, error, nested-call, UI and performance gates remain. |
| Ordinary Linux launcher override | Proof passed | Disposable real launcher selected `.site.json` and created bundled JVM. |
| Workshop subscribe-only native activation | Unresolved | Inspected mod discovery and mounting deliver files without a native activation action. No clean subscribe/launch/update/offline/removal proof. |
| Rust behavior migration | In progress | Actual inventory is 24 profiles and 16 hooks. No bundled profile has an unload hook. |
| In-game browse/add/download/enable/load | Pending | Native API exposure inspected. Actual user workflow not yet verified. |
| General-purpose patcher and two extensions | In progress | Final native capability and independent consumer checks pending. |
| Release, performance, platform execution and review | Pending | Windows and macOS game environments have not been supplied or tested. |

The user explicitly instructed `no claude at all this time`. Implementation and review therefore use GPT models, overriding the repository's model-family requirement for this task.
