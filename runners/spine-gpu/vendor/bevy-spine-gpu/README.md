# bevy_spine_gpu

A GPU-driven [Spine2D](https://esotericsoftware.com/) renderer for [Bevy](https://bevyengine.org/) 0.14.

> **License:** the code is MIT OR Apache-2.0. The art in `assets/` is **not** open source: it is
> © kiek ma games GmbH, all rights reserved, and may only be used to run this repository's tests
> and examples. See [COPYRIGHT](COPYRIGHT) and [assets/LICENSE](assets/LICENSE).

- The vertices of a skeleton type live in one static storage buffer. The vertex shader pulls
  them (no vertex buffers), skins them with the bone transforms of the instance and tints them.
- Each frame, the headers (visible attachments, color, bone offset) and the bone transforms of
  all instances are packed into two storage buffers.
- Each skeleton type is drawn with one instanced draw call. Skeleton types of the *sorted* group
  are drawn in y-sorted order across types (`SkeletonInstance::sort_key`).
- Animation sampling (bones, attachments, events, cross-fades) runs on the CPU; posed bones are
  written straight into the packed instance data.

<!-- TODO benchmark link -->

## Status

Extracted from our game lagateya. This is a reference implementation on Bevy 0.14, not a
maintained crate, and there is no crates.io release.

## Supported Spine features

Spine 4.2 JSON export plus a `.atlas` with one page. Skeletons are limited to 1024 vertices and
96 attachments.

| Feature | Supported |
|---|---|
| Bone rotate / translate / scale timelines, linear / stepped / bezier curves | yes |
| Region attachments, mesh attachments (weighted, up to 4 bones per vertex), linked meshes | yes |
| Attachment timelines, event timelines (`SpineEvent`) | yes |
| Slot setup color times attachment color | yes |
| Multiple skins (one labeled asset `skin/<name>` per skin) | yes |
| Premultiplied-alpha atlases, atlas region offsets / cropping | yes |
| Cross-fade when the animation changes (one track) | yes |
| Bone shear, inherit modes, constraints (IK, transform, path, physics) | no |
| Deform timelines | no |
| Draw order timelines, blend modes | no |
| Slot color timelines, two-color tint | no (the setup color is used, with a warning) |
| Clipping, bounding box, point and path attachments, sequences | no |
| Rotated atlas regions, atlas with several pages, binary `.skel` files | no |
| Rotation of the entity (only translation and scale are followed) | no |
| Interleaving with other 2D content (one item per draw phase in `Transparent2d`) | no |

Slots can be assigned to *draw phases* by name prefix (`SlotRule`, `SpineGPUPlugin::phases`):
every phase draws its slots of all skeletons at its own z, so a phase with a higher z is in
front of the slots of all skeletons in lower phases.

## Usage

```rust
use bevy::prelude::*;
use bevy_spine_gpu::{AnimationControl, LoadSpine, SpineGPUPlugin};

fn main() {
    App::new()
        .add_plugins((DefaultPlugins, SpineGPUPlugin::default()))
        .add_systems(Startup, setup)
        .add_systems(Update, play_animation)
        .run();
}

fn setup(mut commands: Commands) {
    // Loads assets/animations/ella/Ella.json with Ella.atlas next to it
    commands.spawn((
        LoadSpine::spine_directory("animations/ella/Ella"),
        SpatialBundle::default(),
    ));
    commands.spawn(Camera2dBundle::default());
}

// `AnimationControl` is added once the skeleton is loaded
fn play_animation(mut query: Query<&mut AnimationControl, Added<AnimationControl>>) {
    for mut animation in &mut query {
        animation.set_label("walk_l");
    }
}
```

`LoadSpineBundle` / `LoadSpineSettings` set the initial animation, color, time scale, render
bucket and whether the type is drawn in the sorted group.

## Tests and examples

```text
cargo test
cargo run --release --example ella
```

Examples: `ella`, `json_01` to `json_04`, `image` (a static image as a skeleton), `atlas` (an
atlas as a skeleton with one animation per region), `many_ellas`, `varying_animations`,
`draw_order` (sorted group), `bench_instances` (frame time statistics for a mix of types).
Dragging with the left or right mouse button pans, the wheel zooms.

## Correctness

The oracle tests (`src/animation/oracle_tests.rs`) compare this crate's CPU-side pose and skinning
against the official Spine runtime: for every animation of the Ella rig, at sample times across
its duration, they check the world transform of every bone, the skinned position of every vertex
of every visible attachment and which attachment is shown in each slot. The oracle is
[`rusty_spine`](https://crates.io/crates/rusty_spine), bindings to the official spine-c runtime,
which is under the [Spine Runtimes License](https://esotericsoftware.com/spine-runtimes-license).
It is used only as a dev-dependency for these tests and is not part of the crate. The shader
itself is not executed by the tests.

## License

The code is licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in
this crate by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without
any additional terms or conditions.

The art in `assets/` (Spine skeletons, atlases, images) is all rights reserved, see
[assets/LICENSE](assets/LICENSE). It is not covered by the license of the code. [COPYRIGHT](COPYRIGHT)
lists which license applies where.

This is an independent implementation written from Esoteric Software's published format
documentation. It is not affiliated with or endorsed by Esoteric Software. Spine is a trademark of
Esoteric Software. Using the Spine editor and Spine data requires a Spine license.
