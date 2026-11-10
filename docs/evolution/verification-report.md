# Rust migration verification

This report tracks the request preserved in [request.md](request.md). The implementation is complete for the tested Linux runtime path. Overall acceptance remains open while the explicitly listed external gates remain unresolved.

## Tested environment

The run uses Linux x86_64, Project Zomboid `42.22.2 3d39148846`, bundled Azul Java `25.0.5+8-LTS`, and Rust `1.99.1`. The game JAR SHA256 is `3c4a7c5f149d784758c6fb5f8fa85f3a21d572e38bc081400e89035f8120d287`. The native artifact requires GLIBC 2.34 or newer. Other game layouts must match the declared classloader, signatures, and method fingerprints before hooks install.

The original implementation at `84d495a` passed its aggregate offline verification before replacement. That baseline contained 75 portable checks and the original game/JVM integration checks. It establishes the reference behavior, not verification of the new binaries.

## Implemented and exercised

| Area | Evidence scope |
| --- | --- |
| Rust boundary | First-party runtime, patch callbacks, profiles, interface, installer, and release tooling are Rust. The bridge changes one existing host method declaration to native and adds no executable Java/Lua helper. Third-party mod Lua runs in the game's existing runtime. |
| JVM adapter | Actual Kahlua argument and return identity, nested native calls, exception propagation, queued events, owner-thread draining, real UIElement callbacks, classloader rejection, and synthetic native Workshop completion notifications passed with JNI checking and JVM verification. |
| Loader parity | The actual production runtime consumer covers cached nil/false, closure/table identity, native fallback, source overlays and aliases, per-generation execution, profile scan order, dependency failures, scoped replay, callback removal, reconnect, reentrancy, startup readiness, namespace ownership, cleanup refusal, and quarantine. |
| Optional profiles | All 24 bundled profiles and all 16 integration hooks have Rust/declarative replacements. The installed configuration was inspected read-only; its referenced hooks match the recognized migration catalog. No user profiles were modified. |
| Multiplayer | A disposable loopback server retained empty `Mods`/`WorkshopItems`, enabled Lua checksums, and ordinary user roles. All 24 profiles initialized together. Real weapon tooltips, completed AutoAll actions, preference restoration, native inventory/mechanics controls, runtime additions, no-op refresh, and reconnect were exercised. Per-profile isolation and final artifact checks are recorded separately below. |
| Interface | Fresh-menu Options, installed/subscribed/Workshop search, dependency inspection, version details, pending Options protection, file-preserving selection changes, modals, keyboard dismissal, native startup selection, normal relaunch, and patch diagnostics were driven through the real game controls. |
| Workshop reads | An initialized Steam context completed real title search and subscription queries across multiple pages. An unavailable item stayed distinct while healthy results remained usable. Account mutations were not performed. |
| Patcher | Several unrelated real JVM targets passed before hooks, integer replacement and argument modification, priority, nested/concurrent calls, scoped disable, fingerprint/classloader rejection, and preserved host exceptions. Unsupported operations reject explicitly. |
| Extensions | Two independent compiled Rust extensions activated through normal CLI approval records and the production agent. Disabled, unapproved, changed-artifact, changed-manifest, metadata-update, and deferred-target cases passed, including native initializer nonexecution before approval. |
| Installer | Disposable native launcher tests and external CLI checks cover empty defaults, exact legacy hook migration, integrity, interrupted operations, repair, rollback, uninstall, unrelated settings, user edits, permissions, Unicode paths, and immutable templates. |
| Dependency notices | The package retains locked Rust dependency notices and the standard-library notices. Game, JVM, Steam, and Workshop content remain external and are not redistributed. |

## Independent review and corrections

Independent GPT reviews found and reproduced defects in classloader registration, registry reentrancy namespaces, native download completion wiring, failed proof exit codes, callback dependency propagation, tile-definition eligibility, renamed-profile diagnostics, continued-property writes, and profile filename consistency. Their fixes have executable regressions or actual native/UI reruns.

Actual graphical checks additionally found double-instantiated native modals, missing menu polling, description overflow, subscription-page failure propagation, and duplicate native tab IDs after a new foreign tab appeared. The interface now preserves the replaced Mods tab's native ID and position, and the real AllInfo-to-Mods-to-next-load regression passed.

Reference comparison found that the port's initial batch topological sort changed the original profile-file scan order and effective module precedence. The resolver now preserves the original same-pass behavior. A public core consumer and the actual production-runtime consumer check a filename order that differs from mod-ID order and the resulting overlapping-source winner.

## Final acceptance record

| Gate | Current state |
| --- | --- |
| Formatting, strict lint, contract tests, native aggregate | Passed the quiet-tree aggregate against the exact release runtime and CLI: 44 checks passed, with separately identified external workflows not run by that command. |
| Exact release package and two-path reproducibility | Passed after the final ordering correction. Fresh direct-toolchain builds in two paths produced identical binaries, all package contents, and normalized archive bytes. |
| Final native runtime parity | Passed all 19 scenario groups with JNI checking and JVM verification through the exact release-library override. |
| Final multiplayer/isolation matrix | In progress against immutable candidates. Native asset caches are warm across same-process reconnects; checks inspect native file mappings rather than inferring them from cached textures. |
| Matched world performance | In progress under the budget fixed in [performance-budget.md](performance-budget.md). No acceptance result is claimed yet. |
| One-click Workshop activation | Unmet host gate. Inspected Workshop delivery/mounting has no action that starts a newly delivered native agent. A native launcher site override is proven as the additional activation step. |
| Actual subscription/download/update writes | Not run under the user's explicit read-only instruction. Native API binding and notification routing are independently checked, but that is not an account-write end-to-end pass. |
| Physical controller and other OS game execution | Not run. No physical controller or Windows/macOS game environment was supplied. Portable builds do not establish gameplay support. |

Evidence remains outside Git in the implementation machine's LuaBuddy verification state directory. It includes exact artifact hashes, native transcripts, selected diagnostics, screenshots, and the independent counterexamples. The performance outcome, completed isolation roster, and final cleanup state remain pending their deciding runs.

## Release artifact identity

The runtime SHA256 is `832d2d869d648a5bd304e03831df6eca4ce78543301efceabab962141ff0178c`. The installer SHA256 is `20f550a70c23b7976eeaf269bab2fd644097a6808a9201ec0d1dc7b6306d9e66`. The deterministic Linux archive SHA256 is `f28101dc1df66ca31d8558c25ea75b4d26d427e113323b673302ec1a007692a6`. The package manifest independently covers every shipped file.

An online advisory database refresh reported 1,430 advisories and no vulnerabilities or warnings in the workspace lockfile. The offline aggregate also checked each maintained proof and extension lockfile. Offline yanked-crate lookup was explicitly not run.
