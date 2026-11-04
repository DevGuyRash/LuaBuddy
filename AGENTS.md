# LuaBuddy

LuaBuddy's product is a general-purpose late client/shared Lua loader for Project Zomboid. Bundled mod profiles are optional integrations, not the product's default mod list. A fresh installation selects no mods unless the user supplies `--mod` or creates profiles.

## Keep the core small

- Reuse ZombieBuddy for Java instrumentation and Project Zomboid for mod discovery, version selection, dependency metadata, file mounting, translations, and game behavior. Do not build parallel frameworks or copy upstream implementations.
- Keep mod IDs, compatibility rules, and mod-specific initialization in `profiles/`. Do not add mod-specific branches or default selections to the Java loader, Lua bootstrap, or installer.
- Start with an `id=` profile. Add ordering, preloads, mounting, event replay, exclusions, or a Lua hook only when the upstream source or a reproduced failure demonstrates the need. Prefer configuration to code.
- Add a core capability only when an integration needs it and it can serve arbitrary user mods. Make behavior that changes the game environment opt-in and document its limits.
- Read upstream Workshop files in place. Do not vendor mods, edit their files, pin version directories, duplicate their settings, or reimplement their features. Ordinary upstream source updates should need no LuaBuddy change.
- Follow existing idioms and use the smallest complete change. Avoid speculative abstractions, new dependencies, and maintenance work without a current requirement.

## Keep integrations independent

- Support each optional mod on its own, with its actual required dependencies, and alongside other selected mods. Do not assume the bundled profiles are all enabled or that users have only bundled mods.
- Use native dependency metadata first. Use `requires` for real requirements and `before`/`after` for optional coexistence; do not turn an optional companion into a required dependency.
- Preserve AutoAll and AllTheInfo features when adding integrations. Prefer upstream hooks and composable wrappers. Do not disable their features to accommodate another mod or disable another mod's features when the conflicting mod is absent.
- Keep exceptions in the integration that needs them. Avoid broad exclusions, global event triggering, or replacing shared game functions when a narrower upstream mechanism works.
- Preserve user profiles and settings. Adding a bundled profile does not authorize enabling it in a user's existing configuration.
- State late-loading limits honestly: client code cannot provide missing server code, replay the whole startup lifecycle, or register every kind of asset after startup.

## Verify the result

Build the whole change before writing tests. Exercise the user-facing behavior first, then add integration or unit tests only for gaps the earlier checks do not cover. Derive expected results from the request, native APIs, or upstream behavior rather than the implementation under test.

Run `python3 tests/check.py --offline` when the game and verified ZombieBuddy cache are available; otherwise run the portable tests documented in `docs/verification.md` and name the unavailable checks. Integration changes need checks with each affected mod alone and relevant combinations, including AutoAll and AllTheInfo. Use disposable profiles, caches, and servers for game checks; keep personal paths, logs, saves, and downloaded dependencies out of Git. Report the exact verification scope without claiming every mod combination works.

Frontend code, including UI compatibility hooks, must be written and reviewed by a Claude model, with a GPT review in parallel as a supplement.
