# Performance acceptance workload

Record the deciding measurements after this budget is fixed. A successful callback proof or build does not establish acceptable game performance.

Compare a baseline client with a client using the Rust runtime and no selected profiles. Both clients use the same actual game, bundled JVM, private cache, loopback vanilla server, resolution, renderer, player location, and verification instrumentation. This isolates loader overhead from the work performed by third-party mods. Mod-specific behavior and reload/reconnect growth need separate checks.

Alternate at least five baseline and five enabled runs. Record startup duration to the same usable menu or connected-world milestone, frame-time median and 95th/99th percentiles at the actual render boundary, process CPU time, and resident memory. Record errors and completed frame/event counts. A run that fails its behavior check is not a fast successful run.

The initial acceptance budget permits at most five percent additional median startup time, five percent additional frame-time 95th percentile, ten percent additional 99th percentile, and 64 MiB additional steady resident memory for the empty runtime. Treat a timing difference smaller than observed run variation as inconclusive. Record machine load and the limiting workload beside the results.

For lifecycle growth, use a fixed selection and repeated no-op refresh, cooperative reload, and reconnect. Discard the first warmup cycle. Retained JNI callback registrations must follow live host object lifetime, and obsolete module/generation state must become collectible. More than four MiB of repeatable resident growth per settled cycle requires investigation. Resident memory alone cannot prove or disprove retained references.

A narrow Rust verification observer may measure existing host method boundaries on both sides. It must use the same mechanism and sample policy in both arms. A tick interval is not labeled a render-frame time without tracing the actual native render lifecycle. The production runtime does not enable global method tracing.

The budget remains fixed. Measurements and their acceptance status are recorded in [performance.md](performance.md). An inconclusive comparison does not establish a budget pass.

The November 10 hardware follow-up changes the earlier software-rendered fixture before collecting new deciding data. Both arms use the same NVIDIA display and MangoHud presentation observer, a fresh zero-zombie vanilla Steam server, and the same living ordinary player indoors. Native DayLength is set to Real-time, with normal weather policy and clock/weather recorded around the suite. The viewport remains 1280×800 with the native 60 FPS cap and unchanged vsync. This measures overhead within that capped workload, not maximum FPS or an old-implementation speedup. Alternation, warmup, sample duration, numerical limits, and the inconclusive-under-variation rule remain unchanged. The earlier software-rendered results retain their original workload and artifact identity.

The latest hardware attempt stopped after two completed pairs when the user chose to play and work from the collected findings. The remaining six runs were not collected. An incomplete suite cannot pass this budget, and a separate hidden-display feasibility check does not remove resource contention from simultaneous gameplay.
