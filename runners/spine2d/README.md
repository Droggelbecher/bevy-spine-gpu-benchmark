# runners/spine2d

Benchmark runner (`bench-spine2d`, backend id `spine2d`) for [Latias94/spine2d](https://github.com/Latias94/spine2d)
Bevy integration (`spine2d-bevy`) on Bevy 0.19. Standalone workspace; needs Rust >= 1.95 (`rust-toolchain.toml`
pins 1.95.0 for this dir only; rustup installs it automatically).

## Run

```
cargo run --release -- --scene ../../scenes/ella_random.toml --onscreen 1000 [--warmup 3 --duration 15 --no-screenshot --view]
```

## Spine version gate (assets-spine43)

spine2d is a **Spine 4.3-only** runtime. `skeleton.spine` must start with `4.3` (`export_version.rs`, mirrors spine-cpp).
The repo rigs say `4.2-from-4.3.17` (Ella) and `4.2.37` (Mika), so loading the unmodified assets fails:

```
Failed to load asset 'rigs/ella/Ella.json' with asset loader 'spine2d_bevy::asset_loader::JsonSkeletonLoader':
unsupported or invalid Spine version string: 4.2-from-4.3.17
```

Therefore this runner loads from `<repo>/assets-spine43/` by default: a copy of `assets/` where only the version string
is rewritten to `4.3.0` (regenerate with `tools/make-spine43-assets.sh`). The runtime checks nothing else, so the 4.2
JSON parses fine and renders correctly (verified visually); this was accepted as the one documented deviation from
"identical assets". `SPINE2D_ASSET_ROOT=<dir>` overrides the asset root.

## Pinned versions

- `spine2d` 0.5.0 (crates.io, `=0.5.0`), git tag v0.5.0 / commit 16a3434 (no diff in spine2d/spine2d-bevy vs tag).
- `spine2d-bevy` 0.5.0 **vendored** in `vendor/spine2d-bevy` (copy of the v0.5.0 crate) with a 10-line patch:
  `SpineTrackEntrySettings::with_track_time(f32)` (calls the existing `TrackEntryHandle::set_track_time`).
  Needed because the public Bevy API has no way to set the initial track time (phase). Marked `BENCH PATCH`.
  Everything else is unmodified.
- Bevy `=0.19.0` (sub-crates resolve to 0.19.1); recorded in results via Cargo.lock (`bevy_version` column = 0.19.0).

## Implementation

- One entity per instance: `Spine::new(skeleton, atlas)` (default skin) + `Transform` (position, uniform scale).
  All entities share one skeleton and one atlas handle. Assets: `rigs/<rig>/<Rig>.json` / `.atlas` (PNG resolved from atlas).
- Ready: spine2d spawns runtime instances when both assets load and inserts `SpineReady`. A system between
  `SpineSystemSet::Spawn` and `Commands` then sends `SpineAnimationCommand::set_animation(track 0, loop)` with
  `track_time = phase_frac * duration` (duration from `SkeletonData::find_animation`). Loading ends when all instances
  have been sent this command.
- Architecture (relevant to results): each skeleton is an entity with one child `Mesh2d` entity per draw batch
  (per texture/blend run); each frame `update_spine_animations` (single-threaded, NonSend `SpineWorld`) runs animation +
  world transform + mesh building on the main thread and `render_spines` rewrites the CPU `Mesh` assets. So all
  skeleton work counts in `main_ms`; Bevy then extracts/renders thousands of small meshes (`render_cpu_ms`).
- Timings: `frame_ms`/`main_ms` via Instant (`First` before `TimeSystems`, `Last`; `Last` end system is unordered w.r.t.
  other `Last` systems, so main_ms is approximate at the end). `render_cpu_ms`: systems in the render sub-app's `Render`
  schedule, `.before(ExtractCommands)` and `.after(PostCleanup)`, storing the duration (f64 bits) in an `Arc<AtomicU64>`;
  main reads the latest completed value (pipelined rendering, so it is from the previous frame; excludes the Extract
  schedule). `gpu_ms`: `RenderDiagnosticsPlugin` (Vulkan timestamps supported here): sum of `render/<span>/elapsed_gpu`
  for top-level spans (3 path components, e.g. `main_transparent_pass_2d`, `upscaling`), latest value (may lag).
  `SPINE2D_BENCH_LOG_GPU=1` prints the diagnostic paths.
- Window 1920x1080, scale factor override 1.0, non-resizable; `AutoNoVsync` in bench, `AutoVsync` in `--view`;
  `WinitSettings::continuous()`. Screenshot: `Screenshot::primary_window()` + `save_to_disk`, flag set by a second observer.
- `--view`: WASD/arrows pan, wheel zoom (orthographic scale), title shows FPS/ms twice per second.

## Draw order / motion (scene keys `draw_order`, `motion`, `rigs`)

- `rigs`: one `SkeletonData` per rig, instance picks `data[spec.rig]`.
- `y_sorted`: the order is encoded in the skeleton entity's `Transform` z = `bench_core::y_sorted_z(feet_y)` = `-0.5 * y`
  (clamped to +-500; the scale's z is 1, so the children's z is not scaled).
  spine2d-bevy spawns one `Mesh2d` child per draw batch at local z `0.001 * draw_index` (`render.rs`), so a skeleton spans at most
  `0.001 * draws` (a few draws for Ella, scaled by 0.5 through the parent scale) and Bevy sorts by the mesh entity's world z.
  Skeletons 1 px apart in y are 0.5 apart in z, so a whole skeleton lies strictly between its y-neighbours.

## Defaults / knobs noticed (not changed)
- `bob`: `update_motion` (in `Update`) rewrites every instance's `Transform` each frame, so transform propagation runs over the
  whole hierarchy (one entity per bone and per mesh); that is measured.


- No frustum culling of skeleton updates: offscreen instances are still animated and meshed every frame
  (Bevy's own visibility culling may skip drawing them, but the rebuild of their meshes still happens).
- Rendering is Bevy `Mesh2d` + custom materials (one material per blend mode, with/without PMA), no custom batching;
  mesh children are re-spawned when a skeleton's draw signature (texture/blend sequence) changes.
- Skeleton components: `SpineAnimation`, `SpineSkin`, `SpineAnimationStateConfig` (mixes), `SpineSkeletonControl`
  (physics off by default: `Physics::None`), `SpineFlipY`. Possible later variants: physics on, time scale.
- spine2d also has `spine2d-wgpu` (own wgpu renderer, batching) which could be a different render mode to benchmark later.
