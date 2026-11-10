# bevy-spine-direct runner

Benchmarks [jabuwu/bevy_spine PR #42](https://github.com/jabuwu/bevy_spine/pull/42) "Add direct Spine rendering" per `BENCHMARK_SPEC.md`.
Backend id: `bevy_spine_direct`. Binary: `bench-bevy-spine-direct`. Standalone cargo workspace.
Identical to `runners/bevy-spine` (same measurement code, Bevy 0.19.0, toolchain 1.95.0, release profile) except for the two differences below.

## Run

```
cd runners/bevy-spine-direct
cargo run --release -- --scene ../../scenes/ella_random.toml --onscreen 2000 --warmup 1 --duration 3
```

## Pinned version

- `bevy_spine` from git `https://github.com/mirsella/bevy_spine`, rev `edcd1ae0f4dc9e0951ec33f96f02e195bd3a64e6`
  (PR head, branch `spine-direct-3d`).
- Base relationship (checked with the GitHub compare API): the merge base is exactly the baseline runner's rev `c57de4915b3a`
  (0 behind, 2 ahead). The only differences vs the baseline are the PR's two commits, `1f5051db` "feat(render): add direct 3d spine
  rendering" and `edcd1ae0` "feat: gate 3d spine support behind feature". So perf differences are attributable to the PR alone.

## How direct mode is enabled

Per-entity setting: `SpineSettings { direct_rendering: true, ..default() }` is added to each skeleton entity next to `SkeletonDataHandle`
(default is `false`). `SpinePlugin` registers everything else automatically (`SpineDirectRenderPlugin`, direct queue systems for all 8
built-in 2D materials). Nothing else is needed for the 2D default materials; no cargo feature (`3d` not enabled).
All other settings stay default (`drawer = Combined`, `Mesh2D`, `update_meshes_when_invisible = false`, with-children loader).

Mechanism: geometry is written into a per-mesh `SpineDirectMesh` component, extracted to the render world, packed into two shared
vertex/index buffers per frame and uploaded via `write_buffer`; a custom draw command replaces the phase item's draw function. The
backing `Mesh` asset is emptied once and no longer mutated, so no per-frame `AssetEvent::Modified` / `RenderMesh` re-extraction.
Verified active: render CPU at 2000 onscreen drops from ~134 ms to ~13 ms, and the results match the baseline visually.

## Draw order / motion (scene keys `draw_order`, `motion`, `rigs`)

- `rigs`: one `SkeletonData` per rig, instance picks `data[spec.rig]`.
- `y_sorted`: the order is encoded in the skeleton entity's `Transform` z = `bench_core::y_sorted_z(feet_y)` = `-0.5 * y`
  (clamped to +-500; the scale's z is 1, so the children's z is not scaled).
  Same mesh-child z layout as the baseline (0.001 per renderable, at most 0.018 for Ella). Direct rendering still queues the
  items in the normal `Transparent2d` phase (only the draw function is replaced), so the sort key is still the mesh entity's world z.
  Skeletons 1 px apart in y are 0.5 apart in z, so a whole skeleton lies strictly between its y-neighbours (unless feet are within ~0.04 px).

## Caveats
- `bob`: `update_motion` (in `Update`) rewrites every instance's `Transform` each frame, so transform propagation runs over the
  whole hierarchy (one entity per bone and per mesh); that is measured.


- Requires an alpha-blended `Material2d` (fine for Spine defaults); custom materials need `SpineDirectMaterial2dPlugin<M>`.
- Entities get `NoAutomaticBatching`; each combined mesh is still one draw call (one item per mesh, not batched).
- Main-thread cost (~23 ms at 2000) is unchanged: animation update, mesh building and the bone entity hierarchy remain.
- The PR's own benchmark set `update_meshes_when_invisible`; we do not (defaults measured), so offscreen behaviour is the default culling path.
- Windows/Vulkan `AutoNoVsync` may still cap at display rate at low instance counts.
