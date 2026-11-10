# bevy-spine runner

Benchmarks [jabuwu/bevy_spine](https://github.com/jabuwu/bevy_spine) (rusty_spine 0.8) per `BENCHMARK_SPEC.md`.
Backend id: `bevy_spine`. Binary: `bench-bevy-spine`. Standalone cargo workspace.

## Run

```
cd runners/bevy-spine
cargo run --release -- --scene ../../scenes/ella_random.toml --onscreen 1000 --warmup 3 --duration 15
cargo run --release -- --scene ../../scenes/ella_random.toml --view      # interactive
```

`rust-toolchain.toml` pins Rust 1.95.0 here only (bevy_spine main declares `rust-version = "1.95"`; the global default 1.93 is too old).
rustup installs it automatically on first build.

## Pinned versions

- `bevy_spine` **0.12.0** from git rev `c57de4915b3aab447dcdce4279f86930bfb7a5a0` (main, "Update to Bevy 0.19", PR #41).
  The crates.io release 0.10.1 targets older Bevy, so git is required.
- `bevy` **0.19.0** (`default-features = false, features = ["2d"]`; sub-crates resolve to 0.19.1, see Cargo.lock). Reported as `bevy_version = 0.19.0`.
- `rusty_spine` 0.8 (Spine 4.2 runtime). The Spine 4.2 JSON exports for ella and mika load fine, no re-export needed.

## API choices

- `SkeletonData::new_from_json(asset_server.load(json), asset_server.load(atlas))` once per scene; one entity per instance with
  `SkeletonDataHandle` + `Transform` (translation from bench-core, `scale` via Transform scale; skeleton scale untouched) + `InstanceIdx`.
- Readiness: `SpineReadyEvent` handled in `SpineSet::OnReady` (before first animation update). There we call
  `set_animation_by_name(0, anim, true)` and `set_track_time(phase_frac * animation.duration())`. Ready = all events received.
- Default skin, plain PNG atlas (`pma:false`, so bevy_spine's non-PMA materials are used).
- Window 1920x1080 logical, non-resizable, `AssetPlugin.file_path = bench_core::assets_dir()`, `PresentMode::AutoNoVsync`
  (`AutoVsync` in `--view`), `WinitSettings` continuous focused and unfocused, `Camera2d` at origin.

## Timings

- `frame_ms`: `Instant` delta between starts of consecutive `First` schedules (system ordered before `TimeSystems`).
- `main_ms`: `First` start to a system in `Last` (not strictly ordered after other `Last` systems, so slightly under-counts).
- `render_cpu_ms`: render sub-app systems in the `Render` schedule: one `.before(RenderSystems::ExtractCommands)`, one
  `.after(RenderSystems::PostCleanup)`; the elapsed nanos go into an `Arc<AtomicU64>` shared with the main world, which reads the
  latest value each frame. With pipelined rendering (Bevy default) the value belongs to the previous frame's render.
- `gpu_ms`: `RenderDiagnosticsPlugin` (bench mode only). Sum of latest `render/<pass>/elapsed_gpu` diagnostics from `DiagnosticsStore`
  with exactly 3 path components (top-level spans, no nested double counting). Timestamps work on this machine (Vulkan/RTX 5080). The
  value is only the latest diagnostic sample and only covers instrumented passes, so treat it as approximate.
- Screenshot: `Screenshot::primary_window()` + `save_to_disk`, plus a second observer setting a flag after the file is written.

## bevy_spine defaults worth knowing (left untouched)

- `SpineSettings` default: `drawer = Combined` (slots merged into few meshes per skeleton), `mesh_type = Mesh2D`, default materials,
  `update_meshes_when_invisible = false` (meshes of invisible/culled skeletons are not rebuilt; animation state still advances).
- `SpineLoader::with_children()` default: each skeleton spawns one child entity per bone (`SpineBone`) plus mesh child entities. Bone
  entities are not synced (`SpineSync`) but still exist: large entity counts (a big part of its cost).
- Frustum culling is Bevy's normal `Aabb`/visibility on the mesh child entities.

## Draw order / motion (scene keys `draw_order`, `motion`, `rigs`)

- `rigs`: one `SkeletonData` per rig, instance picks `data[spec.rig]`.
- `y_sorted`: the order is encoded in the skeleton entity's `Transform` z = `bench_core::y_sorted_z(feet_y)` = `-0.5 * y`
  (clamped to +-500; the scale's z is 1, so the children's z is not scaled).
  bevy_spine puts each mesh child at local z `0.001 * renderable_index` (`z += 0.001` per renderable slot, per skeleton: Ella has 18
  slots, so at most 0.018) and Bevy sorts `Transparent2d` by the mesh entity's world z. Skeletons 1 px apart in y are 0.5 apart in z,
  so a whole skeleton lies strictly between its y-neighbours unless their feet are within ~0.04 px (stable per frame).

## Caveats
- `bob`: `update_motion` (in `Update`) rewrites every instance's `Transform` each frame, so transform propagation runs over the
  whole hierarchy (one entity per bone and per mesh); that is measured.


- Windows DPI: the 1920x1080 logical window is rendered at the OS scale factor (screenshots are 2880x1620 at 150%). Not overridden.
- `AutoNoVsync` on Windows/Vulkan may still cap at the display rate (50 instances gave ~120 fps, flagged `vsync_suspect`).
- Very slow at scale (2000 onscreen ~4 fps; render CPU thread bound).
