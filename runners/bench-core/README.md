# bench-core

Bevy-independent shared library for the benchmark runners (see `BENCHMARK_SPEC.md`).
Depends only on `serde`, `toml`, `clap`.

- `Args` / `Args::parse_args()` - the common CLI (`--scene --onscreen --offscreen --warmup --duration --no-screenshot --view`).
- `Scene::load(path, &Args)` - parse a scene TOML (`rig` or `rigs`, optional `draw_order`, `motion`), apply overrides, `name` = file stem. Relative paths are tried against the cwd, then the repo root.
- `Scene::instances() -> Vec<InstanceSpec>` - deterministic placement + animation/phase (SplitMix64, two draws per instance).
  `InstanceSpec { index, rig, pos: [f32;3], scale, anim, phase_frac, onscreen, bob_amp, bob_phase }`. Set track time = `phase_frac * anim_duration`.
  `rig` indexes `Scene::rigs` / `rig_paths()`. `pos_at(t)` / `sort_y(t)` give position and feet y at `t` seconds after app start
  (`BenchClock::elapsed_secs`); `y_sorted_z(y)` maps the sort value to a Transform z for the Bevy 0.19 backends.
- `repo_root()`, `assets_dir()`, `results_dir()`, `Scene::rig_paths()` (per rig, asset-relative `rigs/ella/Ella.json`, `.atlas`).
- `screenshot_path(backend, &scene)` - `results/screenshots/<backend>_<scene>.png` (dir created).
- `BenchClock::new(start, &args)`; each frame call `clock.update(Instant::now(), all_ready) -> Phase`
  (`Loading -> Warmup -> Measure -> Screenshot -> Done`). In `Screenshot` (skipped with `--no-screenshot`) request the
  screenshot, then call `clock.screenshot_done()`. Exposes `load_secs`, `measure_secs`.
- `FrameStats::new()`, `push(frame_ms, main_ms, render_cpu_ms, gpu_ms)` only during `Measure`;
  `finish(RunMeta) -> Summary` writes `results/results.csv` (+ header if new), `results/frames/*.csv`,
  and prints a one-line summary.
- `SplitMix64` - the RNG, public for tests.
