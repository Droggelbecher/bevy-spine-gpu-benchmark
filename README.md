# bevy-spine-benchmark

> **License:** the code is MIT OR Apache-2.0. The art in `assets/` and `assets-spine43/` is **not** open
> source: it is © kiek ma games GmbH, all rights reserved, and may only be used to run this repository's
> benchmarks. See [COPYRIGHT](COPYRIGHT) and [assets/LICENSE](assets/LICENSE).

Compares rendering performance of Spine2D renderers for Bevy (desktop only):

| backend id   | crate                                                   | Bevy | runner               |
|--------------|---------------------------------------------------------|------|----------------------|
| `spine-gpu`  | our own `bevy_spine_gpu`, vendored (see `runners/spine-gpu/vendor/VENDORED_FROM.md`) | 0.14 | `runners/spine-gpu`  |
| `bevy_spine` | [jabuwu/bevy_spine](https://github.com/jabuwu/bevy_spine) (rusty_spine) | 0.19 | `runners/bevy-spine` |
| `bevy_spine_direct` | [jabuwu/bevy_spine PR #42](https://github.com/jabuwu/bevy_spine/pull/42) "direct Spine rendering" (fork mirsella, opt-in `direct_rendering`) | 0.19 | `runners/bevy-spine-direct` |
| `spine2d`    | [Latias94/spine2d](https://github.com/Latias94/spine2d) | 0.19 | `runners/spine2d`    |

The contract all runners implement (scene format, placement, RNG, measurements, CSV columns) is in
[BENCHMARK_SPEC.md](BENCHMARK_SPEC.md). Each runner is a standalone cargo workspace; shared, Bevy-independent code
lives in `runners/bench-core`.

## Running

```powershell
# full sweep (builds first), then prints a table
./run_bench.ps1
# subset / instance-count sweep
./run_bench.ps1 -Backends spine-gpu,spine2d -Scenes ella_random,ella_random_offscreen -Counts 500,2000,8000
# with GPU busy time per run from PresentMon (all backends; needs the PresentMon console app, works unelevated)
./run_bench.ps1 -GpuTiming
# table of latest results
./summarize.ps1

# single run (run cargo from inside the runner dir so its rust-toolchain.toml applies)
cd runners/spine2d; cargo run --release -- --scene ../../scenes/ella_random.toml --onscreen 2000

# visual check (vsync on, WASD/arrows pan, wheel zoom, FPS in title, no measuring)
cd runners/spine-gpu; cargo run --release -- --scene ../../scenes/ella_random.toml --view
```

Outputs in `results/`: `results.csv` (one row per run), `frames/` (raw per-frame times), `screenshots/`
(last frame of each run, for checking the backends render the same thing).

## Scenes

| scene                    | what it tests                                              |
|--------------------------|------------------------------------------------------------|
| `ella_same`              | all instances in identical state (best case for batching/caching) |
| `ella_same_phase`        | same animation, random phase                               |
| `ella_random`            | random animation + random phase                            |
| `ella_random_offscreen`  | as above + 10k instances far off-screen (culling)          |
| `mika_random[_offscreen]`| heavier rig (45 bones, 27 slots, 10 meshes)                |
| `ella_random_ysort`      | as `ella_random` + `draw_order = "y_sorted"`               |
| `ella_random_bob`        | as `ella_random` + `motion = "bob"` (everything moves)     |
| `ella_random_ysort_bob`  | y-sorted and moving                                        |
| `mixed3_random_bob`      | rigs `ella`, `ella_b`, `ella_c` (3 skeleton types), moving |
| `mixed3_random_ysort_bob`| 3 skeleton types, y-sorted and moving                      |
| `ella_random_bezier`     | as `ella_random`, every curve a bezier (`tools/make-bezier-rig.py`); not in the default sweep |

Override counts with `--onscreen` / `--offscreen` (or `-Counts` in the sweep script).

Draw order and motion are scene requirements every backend must honour (details in [BENCHMARK_SPEC.md](BENCHMARK_SPEC.md)):
- `draw_order = "any"` (default): z = 0, overlaps are not ordered. spine-gpu draws one instanced batch per skeleton type
  (its static group), so with several types the types are drawn in type order (expected, not a bug).
- `draw_order = "y_sorted"`: of two overlapping instances the one with the lower feet y (lower on screen) is in front, across rigs.
  spine-gpu loads every rig into its *sorted group* and sets `SkeletonInstance::sort_key = -y`; the Bevy 0.19 backends encode
  the order in the `Transform` z (`bench_core::y_sorted_z`: `z = -0.5 * y`; per-backend notes in the runner READMEs).
- `motion = "bob"`: every instance (also offscreen ones) moves vertically, `y = base_y + cell_h * sin(2π (t/4s + phase))`, so
  the y order changes continuously. Updating transforms (and sort keys) every frame is part of what is measured.
- `rigs = [...]` instead of `rig`: instances are spread over several rigs. `ella_b` and `ella_c` are byte-copies of `ella` with
  renamed files, i.e. pure "more skeleton types" stress (worst case for spine-gpu's run building in sorted mode).

Known asset caveats:
- `spine2d` only accepts Spine 4.3 version strings, so it loads `assets-spine43/`: identical to `assets/` except
  `skeleton.spine` is rewritten to `4.3.0` (`tools/make-spine43-assets.sh`).
- `mika_*` scenes are not in the default sweep: spine-gpu doesn't support rotated atlas regions (deferred).
- spine-gpu implements a subset of Spine (feature table and oracle tests against the official runtime in its
  [README](runners/spine-gpu/vendor/bevy-spine-gpu/README.md)). The animations the scenes play (`idle_l/r`,
  `walk_l/r`) use only features it supports; Ella's draw order and deform timelines are only in `walk_up_*`, which no
  scene uses.

## Reading the numbers

- Bevy versions differ: spine-gpu runs on Bevy 0.14.2 (wgpu 0.20), all other backends on Bevy 0.19.0 (wgpu 29), see the
  table at the top. `summarize.ps1` prints a `bevy` column; quote the version next to any cross-backend number.
- Compare `frame_ms_median`/`p99` rather than only FPS.
- `main_ms` ≈ frame time → main-thread (animation/extraction prep) bound; `render_cpu_ms` ≈ frame time → render
  thread bound; neither → likely GPU bound. Draw calls: use Nsight/RenderDoc manually.
- `vsync_suspect = true` means the median sits on a common refresh interval, so the number is likely capped.
  Bevy 0.14 on Windows is known to sometimes stay vsync-capped; raise instance counts until FPS is well below refresh rate.
- `disturbed = true` means focus loss, occlusion, resize/move/scale change, a frame spike (>20x median) or a wrong window size
  was seen during warmup/measure (e.g. the KVM switch toggled the display). See `disturb_reasons`; discard and re-run such rows.
- Two GPU columns, measuring different things:
  - `gpu_ms_mean` (Bevy 0.19 backends only): Bevy's `RenderDiagnosticsPlugin`, timestamp queries around render/compute passes.
  - `gpu_busy_ms` (`-GpuTiming`, every backend): median PresentMon `MsGPUBusy`, the time any GPU engine works for the
    process per frame, so it also includes buffer uploads/copies. Stored in `results/gpu_presentmon.csv` (joined on
    `timestamp` + `backend`); the benchmark process's raw PresentMon frames are kept in `results/presentmon/`.
- Record GPU/driver alongside results manually.

## License

Code: licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your
option. Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this work by
you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.

Art: the Spine rigs in `assets/` and `assets-spine43/` are owned by kiek ma games GmbH, all rights reserved; they may
only be used to run this repository's benchmarks (see [assets/LICENSE](assets/LICENSE));
[COPYRIGHT](COPYRIGHT) lists which license applies where.

Spine is a trademark of Esoteric Software; this project is not affiliated with Esoteric Software. The `bevy_spine` and
`bevy_spine_direct` runners use rusty_spine, i.e. the official spine-c runtime, which is under the
[Spine Runtimes License](https://github.com/EsotericSoftware/spine-runtimes/blob/4.3/LICENSE); using Spine data and
runtimes requires a Spine license.
