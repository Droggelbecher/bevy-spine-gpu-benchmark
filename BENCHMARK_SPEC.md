# Benchmark spec

Contract every backend runner implements, so results are comparable.

## Layout

```
assets/rigs/<rig>/<Rig>.{json,atlas,png}   # identical files for ALL backends (Spine 4.2 JSON, plain PNG, no PMA, no ktx2)
scenes/*.toml                              # scene definitions (data only)
runners/bench-core/                        # Bevy-independent shared lib: CLI, scene parsing, layout, RNG, stats, CSV output
runners/spine-gpu/                         # standalone cargo workspace, Bevy 0.14, vendored spine-gpu under vendor/
runners/bevy-spine/                        # standalone cargo workspace, Bevy 0.19, jabuwu/bevy_spine
runners/spine2d/                           # standalone cargo workspace, Bevy 0.19, Latias94/spine2d
results/                                   # output (gitignored)
run_bench.ps1                              # sweeps scenes x backends
```

Each runner is its own workspace (own Cargo.lock / target dir) because Bevy versions differ. All depend on
`bench-core` via path. `bench-core` must NOT depend on bevy (only std, serde, toml, clap or hand-rolled args).

All runners use the same release profile:
```toml
[profile.release]
opt-level = 3
lto = "thin"
debug = 1
```
Run with `cargo run --release -- <args>`.

## Rigs

| rig  | path                          | anims                              |
|------|-------------------------------|------------------------------------|
| ella | assets/rigs/ella/Ella.json    | idle_l, idle_r, walk_l, walk_r (+dig_*) |
| ella_b, ella_c | assets/rigs/ella_b/Ella_b.json, assets/rigs/ella_c/Ella_c.json | copies of ella with renamed files (json/atlas/png and the atlas page line), so every backend sees distinct skeleton types |
| mika | assets/rigs/mika/Mika.json    | idle_l, idle_r, walk_l, walk_r     |

Use the `default` skin. Do not use spine-gpu sidecar features (no sidecar files are provided).

## Scene file (TOML)

```toml
rig = "ella"                                  # -> assets/rigs/ella/Ella.{json,atlas}
# or: rigs = ["ella", "ella_b", "ella_c"]     # exactly one of rig / rigs
animations = ["idle_l", "idle_r", "walk_l", "walk_r"]
anim_mode = "random"                          # "same" | "same_random_phase" | "random"
onscreen = 1000                               # instances inside the viewport
offscreen = 0                                 # instances far outside the viewport (tests culling)
scale = 0.5                                   # uniform skeleton scale
seed = 42
draw_order = "any"                            # optional: "any" (default) | "y_sorted"
motion = "none"                               # optional: "none" (default) | "bob"
```
Scene name = file stem. CLI can override `onscreen`, `offscreen`. All keys after `seed` are optional; scenes without them
produce exactly the placement/RNG/animation as before they existed.

## Deterministic placement & states (implemented in bench-core, backends just consume it)

- Window: 1920x1080, not resizable, with `WindowResolution::new(1920, 1080).with_scale_factor_override(1.0)` so the
  physical render resolution is 1920x1080 regardless of Windows DPI scaling. 2D camera at origin, default orthographic projection (1 unit = 1 px).
- Onscreen: grid over viewport rect `[-960,960]x[-540,540]` inset by 40 px. `cols = ceil(sqrt(n * w/h))`,
  `rows = ceil(n/cols)`. Instance k sits at its cell's horizontal center, `y = cell_center_y - 0.4*cell_h`
  (skeleton origin is at the feet). z = 0 for all.
- Offscreen: same grid algorithm over the rect centered at (0, 100000) of size 20000x20000 (never visible).
- Instance order: all onscreen first, then offscreen.
- RNG: SplitMix64 seeded with `seed`. For every instance in order draw exactly two values:
  `a = next_u64()`, `p = next_f64()` (in [0,1), from the top 53 bits), regardless of mode.
  - `same`: anim = animations[0], phase_frac = 0
  - `same_random_phase`: anim = animations[0], phase_frac = p
  - `random`: anim = animations[a % len], phase_frac = p
- Rig (with `rigs = [...]`): `rig = rigs[(a >> 32) % len]` (same `a`, no extra RNG draw; a single `rig` is index 0).
  All rigs of a scene must have the listed animations. The `rig` CSV column is the rig names joined with `+`.
- Backends start the animation looping on track 0 and set track time = `phase_frac * animation_duration_secs`.
  Offscreen instances keep animating (don't do anything special for them; whatever culling the backend does by default is what we measure).

## Draw order and motion

- `draw_order = "any"`: z = 0 for all, overlaps are not ordered (spine-gpu: static types, one instanced draw per type).
- `draw_order = "y_sorted"`: an instance whose feet are lower on screen (smaller world y) must be drawn in front of every
  instance it overlaps, across rigs. Ties (equal y) may be broken arbitrarily but must be stable over frames.
  - spine-gpu: load every rig with `with_sorted(true)` (`LoadSpineSettings::sorted`), set `SkeletonInstance::sort_key = -y`.
  - Bevy 0.19 backends: the order is encoded in the instance's `Transform` z, `bench_core::y_sorted_z(y)` = `-0.5 * y`
    (y clamped to +-1000, so z is in [-500, 500], inside the default 2D camera's [-1000, 1000]). The backends add small
    z offsets between a skeleton's slots/meshes (0.001 per slot/draw, <= 0.02 for ella); the mapping is chosen so a whole skeleton
    spans far less z than the gap between skeletons 1 px apart in y (0.5), i.e. slots of different skeletons only interleave
    when their feet are within ~0.04 px. Bevy sorts `Transparent2d` items by the z of the mesh entity.
  - bench-core provides the per-instance sort value: `InstanceSpec::sort_y(t)` (= feet y at time t).
- `motion = "bob"`: `y(t) = base_y + amp * sin(2π * (t / 4 s + bob_phase))`, x fixed, `amp = cell_h` of the instance's grid
  (so neighbouring rows cross each other), `bob_phase = ((a >> 16) & 0xffff) / 65536`, `t` = seconds since app start
  (`BenchClock::elapsed_secs`, Instant-based). Offscreen instances bob too. `InstanceSpec::pos_at(t)` gives the position.
  Every backend updates all instance transforms (and spine-gpu its `sort_key`) each frame in a system in `Update`;
  that cost is part of what is measured.

## Runner lifecycle

1. **Loading**: spawn all instances. Wait until the backend reports every instance ready (skeleton loaded,
   animation applied). Record `load_secs` from app start.
2. **Warmup**: `--warmup` secs (default 3).
3. **Measure**: `--duration` secs (default 15). Record every frame.
4. Take a screenshot to `results/screenshots/<backend>_<scene>.png` (unless `--no-screenshot`), wait
   for it to be written, append results, exit.

## Measurements per frame

- `frame_ms`: wall-clock delta between consecutive frames, measured with `std::time::Instant` in a system at the
  very start of `First` (not Bevy's `Time`).
- `main_ms`: main-world CPU time: Instant at start of `First` -> end of `Last`.
- `render_cpu_ms` (optional but wanted): render-world CPU time for the frame, measured by systems at the first and last
  render sets of the render app's `Render` schedule, sent back to main world (e.g. via a shared `Arc<Mutex<..>>`/atomic).
- `gpu_ms` (optional): if `RenderDiagnosticsPlugin` exists in this Bevy version and GPU timestamps are supported, the
  total GPU time of the frame's render passes. Leave empty if unavailable. Don't sink lots of time into this.

Interpretation: `frame_ms ≈ main_ms` → main-thread CPU bound; `≈ render_cpu_ms` → render-thread bound; neither → GPU bound.

## Output

Append one row to `results/results.csv` (write header if the file is new):

```
timestamp,backend,bevy_version,scene,rig,anim_mode,onscreen,offscreen,load_secs,frames,measure_secs,
fps_mean,frame_ms_mean,frame_ms_median,frame_ms_p95,frame_ms_p99,frame_ms_max,
main_ms_mean,render_cpu_ms_mean,gpu_ms_mean,present_mode,vsync_suspect,
disturbed,disturb_reasons,refresh_hz,window_px
```
(single line in the file.) `fps_mean = frames / measure_secs`. `vsync_suspect = true` if `frame_ms_median` is within
3% of 1000/r for r in {60,75,120,144,165,240}. Also write raw per-frame data to
`results/frames/<backend>_<scene>_<onscreen>_<offscreen>_<timestamp>.csv` (`frame_ms,main_ms,render_cpu_ms,gpu_ms`).
Print a one-line summary to stdout.

Disturbance columns (appended at the end; the machine sits behind a KVM switch, so windows can lose focus, get
occluded, resized or moved during a run). `disturbed = true` if `disturb_reasons` is non-empty; reasons are
joined with `;`. `refresh_hz` = refresh rate of the window's monitor at measure start (empty if unknown; Bevy 0.14
exposes none), `window_px` = physical window size at measure start, e.g. `1920x1080`. Reasons:
- Recorded by the runner only while the phase is Warmup or Measure (loading is excluded): `focus_lost`
  (WindowFocused false), `occluded` (WindowOccluded true), `resized`, `scale_changed` (WindowScaleFactorChanged /
  WindowBackendScaleFactorChanged), `moved`, and `unfocused_at_start` (window not focused at Warmup->Measure).
- Derived by bench-core in `finish()`: `spike` (any frame_ms > 20x the median) and `wrong_size` (window_px != 1920x1080).
- Disturbed rows must not enter comparisons; `summarize.ps1` shows them and warns. If an existing `results.csv` has a
  different header it is renamed to `results.csv.old-<timestamp>` and a fresh file is started.

backend ids: `spine-gpu`, `bevy_spine`, `bevy_spine_direct` (bevy_spine PR #42 with `direct_rendering`), `spine2d`.

## CLI (identical for all runners)

```
--scene <path-to-toml>     required
--onscreen <n>             override
--offscreen <n>            override
--warmup <secs>            default 3
--duration <secs>          default 15
--no-screenshot
--view                     interactive mode: vsync ON, no measuring/exit, WASD/arrow pan, mouse-wheel zoom,
                           FPS + frame ms in window title (updated ~2x/s). For visually checking correctness.
```

## Vsync

Bench mode uses `PresentMode::AutoNoVsync` (fall back to `Immediate`), `WinitSettings` continuous both focused and
unfocused. Known issue: Bevy 0.14 on Windows may still be vsync-capped; `vsync_suspect` flags this, and the workaround is
to use instance counts high enough to fall below refresh rate. Log the present mode actually requested.

## Asset root

Runners set `AssetPlugin { file_path: <absolute path to repo>/assets }`; bench-core exposes `repo_root()` computed from
its own `CARGO_MANIFEST_DIR` (`runners/bench-core/../..`).
