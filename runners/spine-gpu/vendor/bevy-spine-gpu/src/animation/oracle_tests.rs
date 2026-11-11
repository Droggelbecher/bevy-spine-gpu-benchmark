//! Oracle tests: bevy_spine_gpu's CPU-side pose and skinning against the official Spine runtime.
//!
//! The oracle is `rusty_spine` (bindings to the official spine-c 4.2 runtime). For every
//! animation of the Ella rig, at times across `[0, duration]` (every 1/60 s plus every key time
//! and key time +- 1 ms), non-looping, without mixing, the test compares
//!
//! 1. **bones**: the world transform (2x2 matrix and translation) of every bone,
//! 2. **vertices**: the skinned world position of every vertex of every visible attachment,
//!    where bevy_spine_gpu's are computed on the CPU exactly like the vertex shader does it
//!    (`render/spine_gpu.wgsl`, `fn vertex`) from [`VertexInfo`] and the level 1 bone transforms,
//! 3. **visibility**: which attachment is shown in each slot.
//!
//! bevy_spine_gpu's side runs through `compute_pose`, the function of the animation system that fills
//! the GPU buffers (so `pose_bones`, `displayed_local_pose` and the attachment bitmask).
//!
//! # Conventions
//!
//! Both sides use Spine's y-up world space in skeleton units (pixels of the source art), with
//! the skeleton origin at the root bone and no extra scale or offset, so the values are compared
//! directly. bevy_spine_gpu's `Mat2` is column major: `x_axis = (a, c)`, `y_axis = (b, d)` for the
//! Spine matrix `[a b; c d]`. A region attachment has no vertex data in spine-c besides its four
//! corners and bevy_spine_gpu builds a quad of its own, whose corners come in a different order, so
//! regions are matched corner by nearest neighbour. Both sides crop region quads to the atlas
//! region (the whitespace stripped by the packer, see `offsets:` in the `.atlas`).
//!
//! The rigs are the Ella fixture of this crate (6 animations) and the fixture with every bone
//! timeline segment turned into a bezier curve, see [`RIGS`]. Ella itself has no bezier curves,
//! so the second rig covers them: it is derived in memory from the fixture's JSON
//! ([`beziers_everywhere`]), both runtimes load the same modified text. It alternates an ease-in-out shape and an asymmetric one that overshoots
//! the key values, so the value leaves `[v0, v1]` and the x(u) mapping is not the identity.
//!
//! # Known limits and divergences
//!
//! * bevy_spine_gpu renormalizes vertex weights, spine-c does not (see [`TOLERANCE_POSITION`]).
//! * Deform timelines are not supported by bevy_spine_gpu. Ella has them on `Arm R` in `walk_up_l` and
//!   `walk_up_r` ([`KNOWN_DEFORM`]); those vertices are compared separately and expected to
//!   differ, everything else must match.
//! * Draw order timelines, colors, events, IK, transform/path/physics constraints, shear and
//!   inherit modes are out of scope (Ella uses no constraints, shear or inherit modes).
//! * Only the animation system's pose is covered, not the GPU: the WGSL itself is not executed.

use std::{collections::BTreeMap, fmt::Write as _, sync::Arc};

use rusty_spine as rs;

use super::*;
use crate::{Atlas, InstanceInfo, InstanceMut, VertexInfo, loader::build_skeleton_for_tests};

/// A rig to test: its name, the path without extension of the `.json` and `.atlas`, and an
/// optional transformation of the parsed JSON that both runtimes then load.
struct RigSource {
    name: &'static str,
    base: &'static str,
    transform: Option<fn(&mut serde_json::Value)>,
}

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/animations/ella/Ella");

/// The rigs to test: the fixture of this crate, and the fixture with bezier curves on all bone
/// timelines.
const RIGS: &[RigSource] = &[
    RigSource {
        name: "fixture",
        base: FIXTURE,
        transform: None,
    },
    RigSource {
        name: "fixture with beziers",
        base: FIXTURE,
        transform: Some(beziers_everywhere),
    },
];

/// Maximum absolute difference of a world matrix entry (unitless, entries are around 1). f32
/// noise is about 1e-6.
const TOLERANCE_MATRIX: f32 = 1e-5;
/// Maximum difference of a bone's world position in px (Ella is about 500 px tall, f32 noise
/// is about 1e-4 px).
const TOLERANCE_BONE_POSITION: f32 = 1e-3;
/// Maximum difference of a skinned vertex in px. Looser than for bones because bevy_spine_gpu
/// renormalizes vertex weights while spine-c uses them as written: the weights of `Kleid` sum
/// to 1 +- 1e-5, which moves vertices by up to 3e-3 px.
const TOLERANCE_POSITION: f32 = 1e-2;

/// `(animation, slot, attachment)` with a deform timeline, which bevy_spine_gpu does not support.
const KNOWN_DEFORM: &[(&str, &str, &str)] = &[
    ("walk_up_l", "Arm R", "Arm R"),
    ("walk_up_r", "Arm R", "Arm R"),
];

// --------------------------------------------------------------------------------
// Bezier rig
// --------------------------------------------------------------------------------

/// Turns every segment (every key but the last) of every bone timeline that is not stepped into
/// a bezier with absolute control points `[cx1, cy1, cx2, cy2]` per component (Spine 4.x, x
/// component first), alternating two shapes per timeline: ease-in-out, and an asymmetric one
/// that overshoots. Only bone timelines are touched (not `attachments`, so deform data stays).
fn beziers_everywhere(json: &mut serde_json::Value) {
    use serde_json::{Value, json};

    for animation in json["animations"].as_object_mut().unwrap().values_mut() {
        let Some(bones) = animation.get_mut("bones") else {
            continue;
        };
        for timelines in bones.as_object_mut().unwrap().values_mut() {
            for (kind, keys) in timelines.as_object_mut().unwrap() {
                // The value fields of the timeline and their default
                let components: &[(&str, f64)] = match kind.as_str() {
                    "rotate" => &[("value", 0.0)],
                    "translate" => &[("x", 0.0), ("y", 0.0)],
                    "scale" => &[("x", 1.0), ("y", 1.0)],
                    other => panic!("unsupported bone timeline {other}"),
                };
                let keys = keys.as_array_mut().unwrap();
                let number = |key: &Value, field: &str, default: f64| {
                    key.get(field).map_or(default, |v| v.as_f64().unwrap())
                };
                for i in 0..keys.len().saturating_sub(1) {
                    if keys[i].get("curve") == Some(&json!("stepped")) {
                        continue;
                    }
                    let (t0, t1) = (
                        number(&keys[i], "time", 0.0),
                        number(&keys[i + 1], "time", 0.0),
                    );
                    let dt = t1 - t0;
                    let mut curve = Vec::new();
                    for (field, default) in components {
                        let v0 = number(&keys[i], field, *default);
                        let v1 = number(&keys[i + 1], field, *default);
                        let dv = v1 - v0;
                        if i % 2 == 0 {
                            curve.extend([t0 + dt / 4.0, v0, t1 - dt / 4.0, v1]);
                        } else {
                            curve.extend([
                                t0 + dt * 0.1,
                                v0 + dv * 1.4,
                                t0 + dt * 0.6,
                                v1 + dv * 0.3,
                            ]);
                        }
                    }
                    keys[i]["curve"] = json!(curve);
                }
            }
        }
    }
}

// --------------------------------------------------------------------------------
// The two skeletons
// --------------------------------------------------------------------------------

struct Rig {
    gpu: SpineSkeleton,
    /// The vertices of each attachment (`VertexInfo::attachment_index`), in vertex order
    gpu_vertices: BTreeMap<u32, Vec<VertexInfo>>,
    oracle: Arc<rs::SkeletonData>,
    oracle_animations: Arc<rs::AnimationStateData>,
    json: serde_json::Value,
}

impl Rig {
    fn load(source: &RigSource) -> Self {
        let base = source.base;
        let mut json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(format!("{base}.json")).unwrap())
                .unwrap();
        if let Some(transform) = source.transform {
            transform(&mut json);
        }
        let json_text = json.to_string();
        let json = &json_text;
        let atlas_text = std::fs::read_to_string(format!("{base}.atlas")).unwrap();

        let (mut atlas, _page) = Atlas::from_string(&atlas_text).unwrap();
        atlas.set_texture(Handle::default());
        let gpu = build_skeleton_for_tests(&format!("{base}.json"), json, atlas);

        let mut gpu_vertices: BTreeMap<u32, Vec<VertexInfo>> = BTreeMap::new();
        for vertex in &gpu.vertices {
            gpu_vertices
                .entry(vertex.attachment_index)
                .or_default()
                .push(*vertex);
        }

        let oracle_atlas = Arc::new(rs::Atlas::new(atlas_text.as_bytes(), "").unwrap());
        let oracle = Arc::new(
            rs::SkeletonJson::new(oracle_atlas)
                .read_skeleton_data(json.as_bytes())
                .unwrap(),
        );
        let oracle_animations = Arc::new(rs::AnimationStateData::new(oracle.clone()));

        Self {
            gpu,
            gpu_vertices,
            oracle,
            oracle_animations,
            json: serde_json::from_str(json).unwrap(),
        }
    }

    /// Names of the animations, sorted
    fn animation_names(&self) -> Vec<String> {
        let mut names: Vec<_> = self
            .oracle
            .animations()
            .map(|a| a.name().to_owned())
            .collect();
        names.sort();
        names
    }

    fn animation_json(&self, animation: &str) -> &serde_json::Value {
        &self.json["animations"][animation]
    }

    /// Every key time of the animation in the JSON (bones, slots, deform, draw order, ...)
    fn key_times(&self, animation: &str) -> Vec<f32> {
        fn collect(value: &serde_json::Value, times: &mut Vec<f32>) {
            match value {
                serde_json::Value::Object(map) => {
                    if let Some(time) = map.get("time").and_then(|t| t.as_f64()) {
                        times.push(time as f32);
                    }
                    map.values().for_each(|v| collect(v, times));
                }
                serde_json::Value::Array(items) => items.iter().for_each(|v| collect(v, times)),
                _ => {}
            }
        }
        let mut times = Vec::new();
        collect(self.animation_json(animation), &mut times);
        times
    }

    /// `(animation, slot, attachment)` of every deform timeline in the file
    fn deform_timelines(&self) -> Vec<(String, String, String)> {
        let mut found = Vec::new();
        for (animation, content) in self.json["animations"].as_object().unwrap() {
            let Some(skins) = content["attachments"].as_object() else {
                continue;
            };
            for slots in skins.values() {
                for (slot, attachments) in slots.as_object().unwrap() {
                    for (attachment, timelines) in attachments.as_object().unwrap() {
                        if timelines.get("deform").is_some() {
                            found.push((animation.clone(), slot.clone(), attachment.clone()));
                        }
                    }
                }
            }
        }
        found.sort();
        found
    }
}

/// Times to sample: a 1/60 s grid, the end, and each key time with +- 1 ms, clamped to range.
fn sample_times(duration: f32, key_times: &[f32]) -> Vec<f32> {
    let steps = (duration * 60.0).floor() as u32;
    let mut times: Vec<f32> = (0..=steps).map(|i| i as f32 / 60.0).collect();
    times.push(duration);
    for key in key_times {
        times.extend([key - 1e-3, *key, key + 1e-3]);
    }
    for t in &mut times {
        *t = t.clamp(0.0, duration);
    }
    times.sort_by(f32::total_cmp);
    times.dedup();
    times
}

// --------------------------------------------------------------------------------
// Poses
// --------------------------------------------------------------------------------

/// bevy_spine_gpu's pose at time `t`: `compute_pose` as the animation system runs it for a freshly
/// spawned entity at the origin, playing `animation` without transition.
fn gpu_pose(rig: &Rig, animation: &str, t: f32) -> (Vec<GlobalBoneTransform>, UVec3) {
    let skeleton = &rig.gpu.skeleton;
    let mut header = InstanceInfo::default();
    let mut bones = vec![GlobalBoneTransform::default(); skeleton.bones.len()];
    compute_pose(
        InstanceMut {
            header: &mut header,
            bones: &mut bones,
        },
        skeleton,
        Some(&rig.gpu.animations[animation]),
        &AnimationControl::default(),
        &AnimationPlayback::default(),
        &SkeletonInstance::new(Handle::default()),
        &GlobalTransform::IDENTITY,
        t,
        None,
    );
    (bones, header.attachments)
}

/// The official runtime's skeleton with `animation` applied at exactly time `t`.
///
/// `rusty_spine` 0.8 does not expose `Animation::apply`, so this goes through a fresh
/// `AnimationState`: track 0, not looping, track time `t`, no mix (a first entry with nothing
/// to mix from; spine-c applies track 0 with `MixBlend::First`, which at alpha 1 is the setup
/// pose with the timelines on top).
fn oracle_pose(rig: &Rig, animation: &str, t: f32) -> rs::Skeleton {
    let mut skeleton = rs::Skeleton::new(rig.oracle.clone());
    skeleton.set_to_setup_pose();
    let mut state = rs::AnimationState::new(rig.oracle_animations.clone());
    state
        .set_animation_by_name(0, animation, false)
        .unwrap()
        .set_track_time(t);
    state.apply(&mut skeleton);
    skeleton.update_world_transform(rs::Physics::None);
    skeleton
}

/// The attachment of the slot, if it is one of the kinds with vertices (Ella has no others)
fn oracle_attachment_name(slot: &rs::Slot) -> Option<String> {
    slot.mesh_attachment()
        .map(|m| m.name().to_owned())
        .or_else(|| slot.region_attachment().map(|r| r.name().to_owned()))
}

/// World vertices of the slot's current attachment in the official runtime
fn oracle_vertices(slot: &rs::Slot) -> Vec<Vec2> {
    let mut buffer = vec![0.0f32; 2048];
    let n = if let Some(mesh) = slot.mesh_attachment() {
        let n = mesh.world_vertices_length();
        // SAFETY: the attachment is the slot's own
        unsafe { mesh.compute_world_vertices(slot, 0, n, &mut buffer, 0, 2) };
        n as usize / 2
    } else {
        let region = slot.region_attachment().unwrap();
        // SAFETY: as above
        unsafe { region.compute_world_vertices(slot, &mut buffer, 0, 2) };
        4
    };
    buffer[..n * 2]
        .chunks(2)
        .map(|p| Vec2::new(p[0], p[1]))
        .collect()
}

/// What `fn vertex` of `spine_gpu.wgsl` computes
fn skin_vertex(vertex: &VertexInfo, bones: &[GlobalBoneTransform]) -> Vec2 {
    (0..4)
        .map(|i| {
            let bone = &bones[vertex.bone_indices[i] as usize];
            let bind = Vec2::new(vertex.bind_positions_x[i], vertex.bind_positions_y[i]);
            (bone.matrix2() * bind + bone.translation) * vertex.bone_weights[i]
        })
        .sum()
}

fn bit_set(mask: UVec3, index: u32) -> bool {
    [mask.x, mask.y, mask.z][index as usize / 32] & (1 << (index % 32)) != 0
}

/// Names of the attachments of `slot` that bevy_spine_gpu shows
fn gpu_visible_attachments(rig: &Rig, slot: &str, mask: UVec3) -> Vec<String> {
    let info = rig.gpu.info();
    let slot = info.slot_index(slot).unwrap();
    let mut shown: Vec<String> = info
        .attachment_indices_by_slot
        .get(&slot)
        .into_iter()
        .flatten()
        .filter(|(_, index)| bit_set(mask, index.0))
        .map(|(name, _)| name.clone())
        .collect();
    shown.sort();
    shown
}

// --------------------------------------------------------------------------------
// Comparison and statistics
// --------------------------------------------------------------------------------

/// The largest error seen and where
#[derive(Clone, Default)]
struct Worst {
    error: f32,
    at: String,
}

impl Worst {
    fn update(&mut self, error: f32, at: impl FnOnce() -> String) {
        if error > self.error {
            self.error = error;
            self.at = at();
        }
    }
}

/// How the vertices of an attachment are judged
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum VertexClass {
    /// Must match
    Strict,
    /// Has a deform timeline in this animation ([`KNOWN_DEFORM`])
    Deform,
}

fn vertex_class(animation: &str, slot: &str, attachment: &str) -> VertexClass {
    if KNOWN_DEFORM.contains(&(animation, slot, attachment)) {
        VertexClass::Deform
    } else {
        VertexClass::Strict
    }
}

#[derive(Default)]
struct Stats {
    samples: usize,
    matrix: Worst,
    position: Worst,
    /// Vertices that must match
    vertex: Worst,
    deform: Worst,
    visibility_mismatches: Vec<String>,
    vertex_count_mismatches: Vec<String>,
}

/// Compares one animation at one time and adds the errors to `stats`
fn compare_sample(rig: &Rig, animation: &str, t: f32, stats: &mut Stats) {
    let (bones, mask) = gpu_pose(rig, animation, t);
    let oracle = oracle_pose(rig, animation, t);
    let info = rig.gpu.info();
    stats.samples += 1;

    // Level 1: bones
    for bone in oracle.bones() {
        let data = bone.data();
        let name = data.name();
        let index = info.bone_index(name).unwrap().0 as usize;
        let gpu = &bones[index];
        // Spine's [a b; c d] is column major (a, c), (b, d) in glam
        let matrix_error = [
            (gpu.x_axis.x - bone.a()).abs(),
            (gpu.y_axis.x - bone.b()).abs(),
            (gpu.x_axis.y - bone.c()).abs(),
            (gpu.y_axis.y - bone.d()).abs(),
        ]
        .into_iter()
        .fold(0.0, f32::max);
        let position_error = gpu
            .translation
            .distance(Vec2::new(bone.world_x(), bone.world_y()));
        stats
            .matrix
            .update(matrix_error, || format!("{name} t={t:.4}"));
        stats
            .position
            .update(position_error, || format!("{name} t={t:.4}"));
    }

    // Levels 3 and 2: attachments per slot
    for slot in oracle.slots() {
        let slot_name = slot.data().name().to_owned();
        let oracle_name = oracle_attachment_name(&slot);
        let gpu_names = gpu_visible_attachments(rig, &slot_name, mask);
        if gpu_names.first() != oracle_name.as_ref() || gpu_names.len() > 1 {
            stats.visibility_mismatches.push(format!(
                "{animation} {slot_name} t={t:.4}: gpu {gpu_names:?}, spine-c {oracle_name:?}"
            ));
            continue;
        }
        let Some(attachment) = oracle_name else {
            continue;
        };

        let (_, attachment_index) = info
            .slot_attachment_indices(&slot_name, &attachment)
            .unwrap();
        let gpu_vertices = &rig.gpu_vertices[&attachment_index.0];
        let oracle_vertices = oracle_vertices(&slot);
        if gpu_vertices.len() != oracle_vertices.len() {
            stats.vertex_count_mismatches.push(format!(
                "{animation} {slot_name}/{attachment}: gpu {}, spine-c {}",
                gpu_vertices.len(),
                oracle_vertices.len()
            ));
            continue;
        }

        let gpu_positions: Vec<Vec2> = gpu_vertices
            .iter()
            .map(|v| skin_vertex(v, &bones))
            .collect();
        let is_region = slot.region_attachment().is_some();
        let class = vertex_class(animation, &slot_name, &attachment);
        let key = format!("{slot_name}/{attachment}");
        for (i, expected) in oracle_vertices.iter().enumerate() {
            let error = if is_region {
                // The corner order differs: nearest corner
                gpu_positions
                    .iter()
                    .map(|p| p.distance(*expected))
                    .fold(f32::INFINITY, f32::min)
            } else {
                gpu_positions[i].distance(*expected)
            };
            let target = match class {
                VertexClass::Strict => &mut stats.vertex,
                VertexClass::Deform => &mut stats.deform,
            };
            target.update(error, || format!("{key}#{i} t={t:.4}"));
        }
    }
}

fn run_animation(rig: &Rig, animation: &str) -> Stats {
    let duration = rig.oracle.find_animation(animation).unwrap().duration();
    let times = sample_times(duration, &rig.key_times(animation));
    let mut stats = Stats::default();
    for t in times {
        compare_sample(rig, animation, t, &mut stats);
    }
    stats
}

fn rigs() -> impl Iterator<Item = (&'static str, Rig)> {
    RIGS.iter().map(|source| (source.name, Rig::load(source)))
}

// --------------------------------------------------------------------------------
// Tests
// --------------------------------------------------------------------------------

#[test]
fn ella_matches_official_runtime() {
    let mut failures = Vec::new();
    for (rig_name, rig) in rigs() {
        let mut table = String::new();
        writeln!(
            table,
            "{:<11} {:>7} {:>11} {:>9} {:>10}  {:<26} {:>4} {:>9}",
            "animation",
            "samples",
            "bone matrix",
            "bone pos",
            "vertex",
            "worst vertex (slot/att#i)",
            "vis.",
            "deform*"
        )
        .unwrap();

        let mut all = Stats::default();
        for name in rig.animation_names() {
            let stats = run_animation(&rig, &name);
            writeln!(
                table,
                "{:<11} {:>7} {:>11.1e} {:>9.1e} {:>10.1e}  {:<26} {:>4} {:>9.3}",
                name,
                stats.samples,
                stats.matrix.error,
                stats.position.error,
                stats.vertex.error,
                stats.vertex.at.split(" t=").next().unwrap_or_default(),
                stats.visibility_mismatches.len(),
                stats.deform.error,
            )
            .unwrap();

            if stats.matrix.error > TOLERANCE_MATRIX {
                failures.push(format!(
                    "{rig_name} {name}: bone matrix {}",
                    stats.matrix.at
                ));
            }
            if stats.position.error > TOLERANCE_BONE_POSITION {
                failures.push(format!(
                    "{rig_name} {name}: bone position {}",
                    stats.position.at
                ));
            }
            if stats.vertex.error > TOLERANCE_POSITION {
                failures.push(format!("{rig_name} {name}: vertex {}", stats.vertex.at));
            }
            failures.extend(stats.visibility_mismatches.iter().cloned());
            failures.extend(stats.vertex_count_mismatches.iter().cloned());

            // The deform exception must stay a real divergence
            if KNOWN_DEFORM.iter().any(|(a, _, _)| *a == name)
                && stats.deform.error <= TOLERANCE_POSITION
            {
                failures.push(format!(
                    "{rig_name} {name}: deform vertices match, exception obsolete?"
                ));
            }
            all.samples += stats.samples;
            all.matrix.update(stats.matrix.error, || name.clone());
            all.position.update(stats.position.error, || name.clone());
            all.vertex.update(stats.vertex.error, || name.clone());
            all.deform.update(stats.deform.error, || name.clone());
        }
        writeln!(
            table,
            "{:<11} {:>7} {:>11.1e} {:>9.1e} {:>10.1e}  {:<26} {:>4} {:>9.3}",
            "all",
            all.samples,
            all.matrix.error,
            all.position.error,
            all.vertex.error,
            "",
            "",
            all.deform.error,
        )
        .unwrap();
        eprintln!(
            "\nspine-gpu vs spine-c 4.2, Ella ({rig_name}); maximum error over all samples, bones, vertices\n\
             matrix entries unitless, positions in px; tolerances {TOLERANCE_MATRIX:e} (matrix), {TOLERANCE_BONE_POSITION:e} (bones), {TOLERANCE_POSITION:e} (vertices)\n\
             {table}\
             * Arm R in walk_up_*, deform timelines are not supported"
        );
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The deform exceptions are exactly the deform timelines of the files.
#[test]
fn known_deform_exceptions_are_exactly_the_deform_timelines() {
    let mut expected: Vec<_> = KNOWN_DEFORM
        .iter()
        .map(|(a, s, t)| (a.to_string(), s.to_string(), t.to_string()))
        .collect();
    expected.sort();
    for (rig_name, rig) in rigs() {
        assert_eq!(rig.deform_timelines(), expected, "{rig_name}");
    }
}

/// Guards against a vacuous comparison: both skeletons have the same bones and slots in the same
/// order, there is nothing beyond bone timelines to apply, and animating actually moves things.
#[test]
fn oracle_and_gpu_skeletons_correspond() {
    for (rig_name, rig) in rigs() {
        let info = rig.gpu.info();
        assert_eq!(rig.oracle.bones().count(), rig.gpu.skeleton.bones.len());
        for (i, bone) in rig.oracle.bones().enumerate() {
            assert_eq!(info.bone_index(bone.name()), Some(BoneIndex(i as u32)));
        }
        for (i, slot) in rig.oracle.slots().enumerate() {
            assert_eq!(info.slot_index(slot.name()), Some(SlotIndex(i as u32)));
        }
        assert_eq!(rig.oracle.ik_constraints().count(), 0);
        assert_eq!(rig.oracle.transform_constraints().count(), 0);
        assert_eq!(rig.oracle.path_constraints().count(), 0);
        assert_eq!(rig.oracle.physics_constraints().count(), 0);
        for bone in rig.oracle.bones() {
            assert_eq!(bone.shear(), [0.0, 0.0].into(), "{}", bone.name());
            assert!(
                matches!(bone.inherit(), rs::Inherit::Normal),
                "{}",
                bone.name()
            );
        }

        let (setup, _) = gpu_pose(&rig, "walk_l", 0.0);
        let (moved, _) = gpu_pose(&rig, "walk_l", 0.3);
        let max_move = setup
            .iter()
            .zip(&moved)
            .map(|(a, b)| a.translation.distance(b.translation))
            .fold(0.0, f32::max);
        assert!(
            max_move > 5.0,
            "{rig_name}: walk_l barely moves: {max_move}"
        );
    }
}

/// Guards the bezier rig against a vacuous transformation: every segment of every bone timeline
/// is a bezier (or stepped), and the original has none.
#[test]
fn bezier_rig_has_only_bezier_segments() {
    let count = |rig: &Rig| {
        let (mut beziers, mut others) = (0, 0);
        for animation in rig.json["animations"].as_object().unwrap().values() {
            for timelines in animation["bones"].as_object().unwrap().values() {
                for keys in timelines.as_object().unwrap().values() {
                    let keys = keys.as_array().unwrap();
                    for key in &keys[..keys.len() - 1] {
                        match key.get("curve") {
                            Some(c) if c.is_array() => beziers += 1,
                            Some(c) if c == "stepped" => {}
                            _ => others += 1,
                        }
                    }
                }
            }
        }
        (beziers, others)
    };
    let original = Rig::load(&RIGS[0]);
    let bezier = Rig::load(&RIGS[1]);
    assert_eq!(count(&original).0, 0);
    let (beziers, others) = count(&bezier);
    assert!(beziers > 100, "{beziers}");
    assert_eq!(others, 0);
}
