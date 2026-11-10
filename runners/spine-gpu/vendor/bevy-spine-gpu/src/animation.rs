//! Skeletal animation

use std::{collections::HashMap, f32::consts::PI, fmt};

use bevy::{
    math::Affine2,
    prelude::*,
    render::{camera::RenderTarget, primitives::Aabb},
};

mod animation_state;
mod curve;
#[cfg(test)]
mod oracle_tests;
#[cfg(test)]
mod tests;
mod timeline;

pub use animation_state::{AnimationControl, AnimationPlayback};
pub(crate) use curve::{CurveDef, CurveTimeline, KeyDef};
pub(crate) use timeline::{BoneTimelines, Timeline};

use crate::{
    AttachmentIndex, BoneIndex, GlobalBoneTransform, InstanceInfos, InstanceMut,
    MAX_ATTACHMENTS_PER_SKELETON, Skeleton, SkeletonInstance, SpineSkeleton,
};

// --------------------------------------------------------------------------------
// Public API
// --------------------------------------------------------------------------------

/// Default cross-fade time used when changing animation labels.
pub const DEFAULT_ANIMATION_TRANSITION_DURATION: f32 = 0.12;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttachmentOverrideError {
    SlotNotFound { slot: String },
    AttachmentNotFound { slot: String, attachment: String },
}

/// Runtime adjustment of a bone, set from application code via [`AnimationControl::set_bone_override`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BoneOverride {
    /// Added to the bone's local translation (setup + animation), like an additive translate
    /// timeline. Child bones follow.
    pub translate: Vec2,
}

#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SlotIndex(pub(crate) u32);

/// Marker excluding this entity from the viewport-based culling of pose evaluation in
/// [`advance_animations`], so its pose is computed every frame regardless of any camera's
/// position. Playback (time, transitions, [`SpineEvent`]s) advances for all entities anyway;
/// this only matters for what is rendered. Intended for skeletons rendered outside normal world-space
/// camera control (e.g. via a dedicated offscreen camera), where no "main" camera's
/// viewport is a meaningful proxy for whether the entity is visible.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct AlwaysAnimate;

#[derive(Event)]
pub struct SpineEvent {
    pub name: String,
    pub entity: Entity,
}

// --------------------------------------------------------------------------------
// Crate-Internal
// --------------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub(crate) struct AnimationTransition {
    /// Local pose (per bone) at the start of the transition, i.e. what was displayed when the
    /// label changed. Being local, neither entity movement nor the base transform affect it, and
    /// blending it with the target's local pose keeps rotations rigid.
    source_bones: Box<[LocalBoneTransform]>,
    elapsed: f32,
    duration: f32,
}

/// A bone's local transform as translation, rotation and scale, before it is turned into a matrix.
/// Blending happens on this, as blending matrices shrinks and shears rotations.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LocalBoneTransform {
    pub(crate) translation: Vec2,
    /// Radians
    pub(crate) rotation: f32,
    pub(crate) scale: Vec2,
}

impl LocalBoneTransform {
    pub(crate) fn to_affine(self) -> Affine2 {
        Affine2::from_scale_angle_translation(self.scale, self.rotation, self.translation)
    }
}

/// A specific animations timelines (shared part, without instance data)
#[derive(Clone, Debug)]
pub(crate) struct Animation {
    pub(crate) duration: f32,
    pub(crate) bone_timelines: HashMap<BoneIndex, BoneTimelines>,
    pub(crate) slot_timelines: HashMap<SlotIndex, Timeline<AttachmentIndex>>,
    pub(crate) events_timeline: Timeline<String>,
}

// --------------------------------------------------------------------------------
// Impls
// --------------------------------------------------------------------------------

impl fmt::Display for AttachmentOverrideError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AttachmentOverrideError::SlotNotFound { slot } => {
                write!(f, "slot {slot:?} not found")
            }
            AttachmentOverrideError::AttachmentNotFound { slot, attachment } => {
                write!(f, "attachment {attachment:?} not found for slot {slot:?}")
            }
        }
    }
}

impl std::error::Error for AttachmentOverrideError {}

// --------------------------------------------------------------------------------
// Systems
// --------------------------------------------------------------------------------

pub(crate) fn compute_setup_pose(
    mut instances: Query<(Entity, &SkeletonInstance, &GlobalTransform), Added<SkeletonInstance>>,
    mut instance_infos: ResMut<InstanceInfos>,
    skeletons: Res<Assets<SpineSkeleton>>,
) {
    for (entity, skeleton_ref, gt) in &mut instances {
        let Some(InstanceMut {
            header: instance_info,
            bones,
        }) = instance_infos.get_mut(entity)
        else {
            warn!("New skeleton instance not found in instance_infos, not computing setup pose!");
            continue;
        };

        let Some(SpineSkeleton { skeleton, .. }) = skeletons.get(&skeleton_ref.skeleton) else {
            warn!("New skeleton not found, not computing setup pose!");
            continue;
        };

        let entity_transform = entity_affine(gt);
        pose_bones(
            bones,
            skeleton,
            skeleton_ref.base_transform,
            entity_transform,
            |i| combined_bone_transform(&skeleton.bones[i], None, 0.0, Vec2::ZERO).to_affine(),
        );

        // Compute current attachment state

        instance_info.attachments = UVec3::ZERO;
        for default_attachment in skeleton.default_attachments.iter() {
            uvec3_set_bit(&mut instance_info.attachments, default_attachment.0);
        }

        instance_info.color = skeleton_ref.color;
    }
}

/// Advance playback of all skeletons and evaluate the pose of the visible ones.
///
/// Playback (label sync, time, transitions, events) is advanced for every skeleton, whether it
/// is on screen or not, so systems listening to [`SpineEvent`]s behave the same regardless
/// of the camera. Only the pose evaluation (bones, attachments, instance info) is skipped for
/// entities outside the camera's view, unless they are [`AlwaysAnimate`]. Without any camera,
/// nothing is rendered and only playback advances (plus the pose of `AlwaysAnimate` entities).
pub(crate) fn advance_animations(
    mut instances: Query<(
        Entity,
        &AnimationControl,
        &mut AnimationPlayback,
        &SkeletonInstance,
        &GlobalTransform,
        &Aabb,
        Has<AlwaysAnimate>,
    )>,
    time: Res<Time>,
    mut instance_infos: ResMut<InstanceInfos>,
    skeletons: Res<Assets<SpineSkeleton>>,
    cameras: Query<(&Camera, &OrthographicProjection, &GlobalTransform)>,
    mut events: EventWriter<SpineEvent>,
) {
    // Culling is done against the window-rendering camera's viewport. Other
    // cameras (e.g. one rendering into an offscreen texture) may coexist but shouldn't
    // affect it.
    let cam_area = cameras
        .iter()
        .find(|(camera, _, _)| matches!(camera.target, RenderTarget::Window(_)))
        .or_else(|| cameras.iter().next())
        .map(|(_, projection, cam_gt)| {
            let (scale, _, tr) = cam_gt.to_scale_rotation_translation();
            Rect {
                min: projection.area.min * scale.xy() + tr.xy(),
                max: projection.area.max * scale.xy() + tr.xy(),
            }
        });

    let delta = time.delta_seconds();
    let mut n_computed = 0;

    for (entity, control, mut playback, skeleton_ref, gt, aabb, always_animate) in &mut instances {
        let Some(SpineSkeleton {
            skeleton,
            animations,
            ..
        }) = skeletons.get(&skeleton_ref.skeleton)
        else {
            continue;
        };

        // Playback: for every entity, visible or not
        // --------------------------------------------------------------------------------

        sync_label(&mut playback, control, skeleton, animations);

        let maybe_animation = animations.get(&playback.label);
        if maybe_animation.is_none() && !playback.label.is_empty() {
            warn_once!(
                "Animation {:?} not found for entity {:?}. Have: {:?}",
                playback.label,
                entity,
                animations.keys().collect::<Vec<_>>()
            );
        }

        let (prev_time, time) = advance_time(&mut playback, control, maybe_animation, delta);
        let transition_factor = advance_transition(&mut playback, delta);

        if let Some(a) = maybe_animation {
            for (_, name) in a.events_timeline.iter_range(prev_time, time) {
                events.send(SpineEvent { entity, name });
            }
        }

        // Pose: only for visible entities
        // --------------------------------------------------------------------------------

        let visible = always_animate
            || cam_area.is_some_and(|cam_area| {
                !cam_area
                    .intersect(Rect {
                        min: aabb.min().xy(),
                        max: aabb.max().xy(),
                    })
                    .is_empty()
            });

        if visible && let Some(instance) = instance_infos.get_mut(entity) {
            compute_pose(
                instance,
                skeleton,
                maybe_animation,
                control,
                &playback,
                skeleton_ref,
                gt,
                time,
                transition_factor,
            );
            n_computed += 1;
        }

        // Dropped only now: the pose above still needs the final factor of the transition.
        // A finished transition is fully on the target pose, so dropping it unseen is no pop.
        if transition_factor.is_some_and(|factor| factor >= 1.0) {
            playback.transition = None;
        }
    }

    if n_computed != 0 {
        debug!("computed {:?} animations", n_computed);
    }
}

/// Sync the label from `control`, starting a transition when it changed. Only touches
/// `playback` mutably (and clones) when it changed.
///
/// The transition's source is captured here, as the local pose that was displayed last frame
/// (including a still running transition), so that rapid label changes do not pop.
fn sync_label(
    playback: &mut Mut<AnimationPlayback>,
    control: &AnimationControl,
    skeleton: &Skeleton,
    animations: &HashMap<String, Animation>,
) {
    if playback.label != control.label {
        let duration = control.label_transition.max(0.0);
        let playback = &mut **playback;

        let transition = (duration > 0.0).then(|| {
            // Looked up by the old label, before it is overwritten
            let old_animation = animations.get(&playback.label);
            let running = playback
                .transition
                .as_ref()
                .map(|t| (&*t.source_bones, (t.elapsed / t.duration).clamp(0.0, 1.0)));
            let source_bones = (0..skeleton.bones.len())
                .map(|i| {
                    displayed_local_pose(
                        i,
                        skeleton,
                        old_animation,
                        control,
                        playback.time,
                        running,
                    )
                })
                .collect();
            AnimationTransition {
                source_bones,
                elapsed: 0.0,
                duration,
            }
        });

        playback.label.clone_from(&control.label);
        playback.transition = transition;
    }
}

/// Advance and wrap the playback time. Returns `(prev_time, time)`, the (wrapped) range
/// the time moved through, for emitting events.
fn advance_time(
    playback: &mut AnimationPlayback,
    control: &AnimationControl,
    animation: Option<&Animation>,
    delta: f32,
) -> (f32, f32) {
    let mut prev_time = playback.time;

    if let Some(animation) = animation {
        prev_time = wrap_animation_time(prev_time, animation.duration, control.repeat);
        playback.time = wrap_animation_time(
            prev_time + delta * control.time_scale,
            animation.duration,
            control.repeat,
        );
    }
    (prev_time, playback.time)
}

/// Advance the running transition, if any. Returns its blend factor (`>= 1.0` once finished).
/// A finished transition is not dropped here, see [`advance_animations`].
fn advance_transition(playback: &mut AnimationPlayback, delta: f32) -> Option<f32> {
    let transition = playback.transition.as_mut()?;
    transition.elapsed = (transition.elapsed + delta).min(transition.duration);
    Some((transition.elapsed / transition.duration).clamp(0.0, 1.0))
}

/// Compute bones, attachments and colors of a visible skeleton instance into `info`.
#[allow(clippy::too_many_arguments)]
fn compute_pose(
    InstanceMut {
        header: info,
        bones,
    }: InstanceMut,
    skeleton: &Skeleton,
    maybe_animation: Option<&Animation>,
    control: &AnimationControl,
    playback: &AnimationPlayback,
    skeleton_ref: &SkeletonInstance,
    gt: &GlobalTransform,
    time: f32,
    transition_factor: Option<f32>,
) {
    let gt_affine = entity_affine(gt);

    let running = transition_factor
        .zip(playback.transition.as_ref())
        .map(|(factor, transition)| (&*transition.source_bones, factor));

    pose_bones(
        bones,
        skeleton,
        skeleton_ref.base_transform,
        gt_affine,
        |i| displayed_local_pose(i, skeleton, maybe_animation, control, time, running).to_affine(),
    );

    // Compute current attachment state
    // --------------------------------------------------------------------------------

    info.attachments = UVec3::ZERO;

    for (i, default_attachment) in skeleton.default_attachments.iter().enumerate() {
        let slot = SlotIndex(i as u32);
        let animated_attachment =
            if let Some(timeline) = maybe_animation.and_then(|a| a.slot_timelines.get(&slot)) {
                timeline
                    .get_latest_value(time)
                    .unwrap_or(*default_attachment)
            } else {
                *default_attachment
            };
        let attachment = control
            .attachment_overrides
            .get(&slot)
            .copied()
            .unwrap_or(animated_attachment);

        uvec3_set_bit(&mut info.attachments, attachment.0);
    }

    info.color = skeleton_ref.color;
}

// --------------------------------------------------------------------------------
// Helper functions
// --------------------------------------------------------------------------------

/// The part of an entity's transform that its skeleton follows: translation and scale.
///
/// Rotation of the entity is ignored: skeletons cannot be rotated.
pub(crate) fn entity_affine(gt: &GlobalTransform) -> Affine2 {
    let (scale, _rotation, translation) = gt.to_scale_rotation_translation();
    Affine2::from_scale_angle_translation(scale.xy(), 0.0, translation.xy())
}

/// Compute the global bone transforms of `skeleton` into `global`, given each bone's local
/// transform.
///
/// The root bone is `entity_transform * base_transform * bone_transform(0)`, every other bone is
/// its parent's transform times its local one. The root bone lives in skeleton space like all
/// other bones, so e.g. a root `scaleX = -1` (the usual way to face right) mirrors about the
/// skeleton origin, not about the entity origin the base transform offsets the art from. The setup pose and the animated pose both go
/// through here, so they always agree.
fn pose_bones(
    global: &mut [GlobalBoneTransform],
    skeleton: &Skeleton,
    base_transform: Affine2,
    entity_transform: Affine2,
    bone_transform: impl Fn(usize) -> Affine2,
) {
    global[0] = (entity_transform * base_transform * bone_transform(0)).into();
    for i in 1..skeleton.bones.len() {
        let p = skeleton.bones[i].parent.0 as usize;
        global[i] = global[p] * bone_transform(i);
    }
}

/// The local pose of bone `i` as displayed: the animated pose at `time`, blended from the
/// transition's source pose if one is running (`(source, factor)`).
fn displayed_local_pose(
    i: usize,
    skeleton: &Skeleton,
    animation: Option<&Animation>,
    control: &AnimationControl,
    time: f32,
    transition: Option<(&[LocalBoneTransform], f32)>,
) -> LocalBoneTransform {
    let timelines = animation.and_then(|a| a.bone_timelines.get(&BoneIndex(i as u32)));
    let translate_offset = control
        .bone_overrides
        .get(&BoneIndex(i as u32))
        .map_or(Vec2::ZERO, |o| o.translate);
    let local = combined_bone_transform(&skeleton.bones[i], timelines, time, translate_offset);
    match transition {
        Some((source, factor)) => blend_bone_transform(source[i], local, factor),
        None => local,
    }
}

/// Combine setup-pose and animation deltas into a single local bone transform.
///
/// Spine semantics: rotation and position are additive, scale is multiplicative.
/// Constructing the matrix after combining ensures correct behaviour under non-uniform
/// and negative scale (e.g. `S(-1,1) * R(dr) = R(-dr) * S(-1,1)` so matrix
/// multiplication would reverse the animation rotation on mirrored bones).
///
/// `translate_offset` is a runtime override ([`BoneOverride`]), added like an animated translation.
fn combined_bone_transform(
    bone: &crate::skeleton::Bone,
    timelines: Option<&BoneTimelines>,
    time: f32,
    translate_offset: Vec2,
) -> LocalBoneTransform {
    let (anim_rot_deg, anim_pos, anim_scale) = match timelines {
        Some(tl) => (
            tl.get_rotation(time),
            tl.get_translation(time),
            tl.get_scale(time),
        ),
        None => (0.0, Vec2::ZERO, Vec2::ONE),
    };
    LocalBoneTransform {
        translation: bone.setup_position + anim_pos + translate_offset,
        rotation: bone.setup_rotation + anim_rot_deg * PI / 180.0,
        scale: bone.setup_scale * anim_scale,
    }
}

/// Blend two local bone transforms: translation linearly, rotation along the shortest way,
/// scale by magnitude.
fn blend_bone_transform(
    from: LocalBoneTransform,
    to: LocalBoneTransform,
    factor: f32,
) -> LocalBoneTransform {
    let factor = factor.clamp(0.0, 1.0);

    // Shortest way around: the difference wrapped into [-PI, PI)
    let delta = (to.rotation - from.rotation + PI).rem_euclid(2.0 * PI) - PI;

    // In order to avoid paper-mario like "flips" when changing walking direction (facing is
    // mirrored by a negative scale, and lerping -1 to 1 would pass through 0), blend the
    // magnitudes and flip the sign abruptly halfway through.
    let sign_source = if factor < 0.5 { from.scale } else { to.scale };
    let scale = Vec2::new(
        from.scale.x.abs().lerp(to.scale.x.abs(), factor) * sign_source.x.signum(),
        from.scale.y.abs().lerp(to.scale.y.abs(), factor) * sign_source.y.signum(),
    );

    LocalBoneTransform {
        translation: from.translation.lerp(to.translation, factor),
        rotation: from.rotation + delta * factor,
        scale,
    }
}

fn wrap_animation_time(time: f32, duration: f32, repeat: bool) -> f32 {
    if time <= 0.0 || duration <= 0.0 {
        return 0.0;
    }

    if !repeat {
        return time;
    }

    time.rem_euclid(duration)
}

/// Set bit `idx` of the 96-bit attachment visibility mask.
///
/// `load_skeleton` rejects skeletons with too many attachments, so `idx` out of range should
/// not happen for loaded skeletons. Should it happen anyway (e.g. a hand-built skeleton), we
/// skip the bit with a warning instead of panicking in the middle of a frame; the attachment
/// is then merely invisible rather than a different attachment toggling silently.
fn uvec3_set_bit(v: &mut UVec3, idx: u32) {
    if idx == AttachmentIndex::INVALID.0 {
        return;
    }

    if idx as usize >= MAX_ATTACHMENTS_PER_SKELETON {
        warn_once!(
            "Attachment index {idx} exceeds the limit of {MAX_ATTACHMENTS_PER_SKELETON}, attachment will not be shown"
        );
        return;
    }

    if idx < 32 {
        v.x |= 1 << idx;
    } else if idx < 2 * 32 {
        v.y |= 1 << (idx - 32);
    } else {
        v.z |= 1 << (idx - 2 * 32);
    }
}
