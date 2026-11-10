# spine-gpu runner (Bevy 0.14)

Benchmarks the `bevy_spine_gpu` crate (GPU-skinned Spine renderer) per `BENCHMARK_SPEC.md`.
Standalone cargo workspace; the crate is vendored under `vendor/bevy-spine-gpu/` (see `vendor/VENDORED_FROM.md` for
the source commit and its history).

## Run

```
cd runners/spine-gpu
cargo run --release -- --scene ../../scenes/ella_random.toml --onscreen 1000 --warmup 3 --duration 15
cargo run --release -- --scene ../../scenes/ella_random.toml --view      # interactive: WASD/arrows, wheel zoom
```
Flags: `--scene --onscreen --offscreen --warmup --duration --no-screenshot --view` (from `bench-core`).
Output: `results/results.csv`, `results/frames/*.csv`, `results/screenshots/spine-gpu_<scene>.png` in the repo root.

## Loader / API notes

- `LoadSpine::spine_directory("rigs/ella/Ella")` (no extension): spine-gpu loads the `SpineSkeleton` asset
  `rigs/ella/Ella.json` with its `.atlas` sibling and the atlas page image (`Ella.png`, as named inside the atlas),
  all asynchronously through the Bevy `AssetServer` (`AssetPlugin.file_path` = `bench_core::assets_dir()`). Our plain
  files work as-is; no padded/pmafix/ktx2 variants are needed, no sidecar (`.meta.toml`) is present so defaults apply.
- spine-gpu caches the skeleton type (a `Handle<SpineSkeleton>`) per `LoadSpine` value, so all instances of a rig
  share one skeleton/vertex buffer/atlas and are drawn with GPU instancing; each entity carries `SkeletonInstance`,
  `AnimationControl` and `AnimationPlayback`.
- Animation: `LoadSpineSettings { animation, transition_duration: 0 }` sets the label (playback starts on the same
  label, so no cross-fade). Once the instance is loaded, `AnimationPlayback::set_current_time_seconds(phase_frac *
  duration)` seeks, with the duration from the type's `SkeletonInfo` (`SkeletonInstance::info`). Looping is the
  default (`AnimationControl::set_repeat(true)`); `--freeze` sets `AnimationControl::set_time_scale(0.0)`.
- Scale is applied via `Transform::from_scale`.
- Readiness = every instance has been configured, i.e. has its `SkeletonInstance` / `AnimationPlayback`, which
  spine-gpu inserts only once the skeleton *and* its texture are loaded.
- Failed loads are only logged by spine-gpu (no panic). The runner keeps its own handles to the rigs' `SpineSkeleton`
  assets and exits with an error (no CSV row) when one fails, instead of waiting in the loading phase forever.
- Culling: spine-gpu skips pose evaluation for skeletons whose padded AABB is outside the camera view (playback time
  still advances for all of them); offscreen instances are therefore cheap, which is what the offscreen scenes measure.
  Culled instances are still uploaded and drawn (with their last pose).

## Draw order / motion (scene keys `draw_order`, `motion`, `rigs`)

- spine-gpu has two draw groups. `draw_order = "any"`: default static group, one instanced draw per skeleton type
  (several types are drawn in type order). `y_sorted`: every rig is loaded with `LoadSpineSettings { sorted: true, .. }` (a
  property of the skeleton type), and each instance's `SkeletonInstance::sort_key = -y` (feet y, from `bench_core`); the render
  world keeps the order across frames, re-sorts and draws one `draw_indexed` per run of consecutive same-type instances.
- No z is used (instances stay at z = 0); `Transform` is only moved for `motion = "bob"`.
- `update_motion` (in `Update`, before spine-gpu's `PostUpdate` systems) sets the bob transform and `sort_key` every frame; for static
  `y_sorted` scenes the key is set once when `SkeletonInstance` appears. Writing `sort_key` marks `SkeletonInstance` changed (also an
  upload/extract cost, intended: it is part of the sorted-moving price).
- Each rig path is its own `LoadSpine` value and so its own cached skeleton type (`ella_b`/`ella_c` are separate types).

## Timings

- `frame_ms`: `Instant` delta at start of `First` (before Bevy's time system).
- `main_ms`: start of `First` to `Last` (system in `Last`; may miss a few trailing schedule-end bookkeeping items).
- `render_cpu_ms`: render sub-app systems in `RenderSet::ExtractCommands` (start) and `RenderSet::Cleanup`
  (end) of the `Render` schedule, sent back via `Arc<AtomicU64>`. With pipelined rendering the render frame N overlaps
  main frame N+1, so each sample is the most recently *completed* render frame (lags by one). It excludes the
  extract step (runs on the main thread, counted in `main_ms`/`frame_ms`) and time the render thread spends blocked.
- `gpu_ms`: not supported (Bevy 0.14 has no `RenderDiagnosticsPlugin`); column left empty.
- `bevy_version` is parsed from `Cargo.lock` at compile time.

## Caveats

- Bevy 0.14 on Windows may stay vsync-capped even with `AutoNoVsync`; check `vsync_suspect` in the CSV.
- Bevy 0.14 with its default features.
- Window resolution uses `with_scale_factor_override(1.0)` so the physical render size is exactly 1920x1080 regardless of OS DPI scaling.

## Known issues / results notes

- **Mika does not load**: `assets/rigs/mika/Mika.atlas` contains 10 regions with `rotate:90` (atlas packer rotation).
  `pma:true`. spine-gpu's atlas loader explicitly bails ("Region rotation is not supported"); the load fails and the
  runner exits with an error. The atlas would need to be re-exported with rotation disabled;
  vendored code was not changed. `mika_*` scenes therefore do not produce a row for this backend.
- Vsync: on the dev machine (RTX 5080, Vulkan, Windows 11) `AutoNoVsync` was honoured (1487 fps at 50 instances,
  `vsync_suspect=false` at every count tried).
