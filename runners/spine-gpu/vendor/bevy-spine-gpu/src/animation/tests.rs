use bevy::ecs::system::RunSystemOnce as _;

use super::*;
use crate::{SkeletonInfo, SpineSkeleton};

#[test]
fn set_bit_covers_all_three_words() {
    let mut v = UVec3::ZERO;
    for idx in [0, 31, 32, 63, 64, 95] {
        uvec3_set_bit(&mut v, idx);
    }
    assert_eq!(v, UVec3::new(1 | 1 << 31, 1 | 1 << 31, 1 | 1 << 31));
}

#[test]
fn set_bit_ignores_invalid_and_out_of_range() {
    let mut v = UVec3::ZERO;
    uvec3_set_bit(&mut v, AttachmentIndex::INVALID.0);
    uvec3_set_bit(&mut v, MAX_ATTACHMENTS_PER_SKELETON as u32);
    uvec3_set_bit(&mut v, MAX_ATTACHMENTS_PER_SKELETON as u32 + 1);
    assert_eq!(v, UVec3::ZERO);
}

fn skeleton_info_with_tool_slot() -> SkeletonInfo {
    let slot = SlotIndex(3);
    let pickaxe = AttachmentIndex(7);
    let hammer = AttachmentIndex(8);

    SkeletonInfo {
        slot_indices_by_name: HashMap::from([("tool".to_string(), slot)]),
        attachment_indices_by_slot: HashMap::from([(
            slot,
            HashMap::from([
                ("pickaxe".to_string(), pickaxe),
                ("hammer".to_string(), hammer),
            ]),
        )]),
        ..default()
    }
}

#[test]
fn sets_and_clears_attachment_override_resolved_by_name() {
    let info = skeleton_info_with_tool_slot();
    let mut state = AnimationControl::default();
    let (slot, hammer) = info.slot_attachment_indices("tool", "hammer").unwrap();

    state.set_slot_attachment(slot, hammer);
    assert_eq!(state.slot_attachment_override(slot), Some(hammer));

    state.clear_slot_attachment_override(slot);
    assert_eq!(state.slot_attachment_override(slot), None);
}

#[test]
fn reports_unknown_slot_or_attachment() {
    let info = skeleton_info_with_tool_slot();

    assert_eq!(
        info.slot_attachment_indices("hand", "hammer"),
        Err(AttachmentOverrideError::SlotNotFound {
            slot: "hand".to_string()
        })
    );
    assert_eq!(
        info.slot_attachment_indices("tool", "saw"),
        Err(AttachmentOverrideError::AttachmentNotFound {
            slot: "tool".to_string(),
            attachment: "saw".to_string()
        })
    );
}

fn test_bone(parent: BoneIndex, position: Vec2) -> crate::skeleton::Bone {
    crate::skeleton::Bone {
        parent,
        setup_rotation: 0.0,
        setup_position: position,
        setup_scale: Vec2::ONE,
    }
}

/// Global position of a child at local (10, 0) under a parent at local (1, 2), applying
/// `state`'s override for the parent (bone 0) the way `advance_animations` does.
fn child_global_position(
    state: &AnimationControl,
    parent_timelines: Option<&BoneTimelines>,
    time: f32,
) -> Vec2 {
    let parent = test_bone(BoneIndex::INVALID, Vec2::new(1.0, 2.0));
    let child = test_bone(BoneIndex(0), Vec2::new(10.0, 0.0));
    let offset = state
        .bone_override(BoneIndex(0))
        .map_or(Vec2::ZERO, |o| o.translate);
    let parent_global =
        combined_bone_transform(&parent, parent_timelines, time, offset).to_affine();
    let child_global =
        parent_global * combined_bone_transform(&child, None, time, Vec2::ZERO).to_affine();
    child_global.translation
}

fn info_with_parent_bone() -> SkeletonInfo {
    SkeletonInfo::with_names_for_tests(&[], &["parent"])
}

fn translate_timeline(value: [f32; 2]) -> BoneTimelines {
    BoneTimelines {
        translate: CurveTimeline::new([KeyDef {
            time: 0.0,
            value,
            curve: [CurveDef::Linear; 2],
        }]),
        ..default()
    }
}

#[test]
fn bone_override_moves_child_bones() {
    let info = info_with_parent_bone();
    let mut state = AnimationControl::default();
    assert_eq!(
        child_global_position(&state, None, 0.0),
        Vec2::new(11.0, 2.0)
    );

    state.set_bone_override(
        info.bone_index("parent").unwrap(),
        BoneOverride {
            translate: Vec2::new(0.0, -5.0),
        },
    );
    assert_eq!(
        child_global_position(&state, None, 0.0),
        Vec2::new(11.0, -3.0)
    );
}

#[test]
fn bone_override_is_additive_with_animation() {
    let info = info_with_parent_bone();
    let mut state = AnimationControl::default();
    let timelines = translate_timeline([3.0, 4.0]);
    assert_eq!(
        child_global_position(&state, Some(&timelines), 0.0),
        Vec2::new(14.0, 6.0)
    );

    let bone = info.bone_index("parent").unwrap();
    state.set_bone_override(
        bone,
        BoneOverride {
            translate: Vec2::new(0.0, -5.0),
        },
    );
    assert_eq!(
        child_global_position(&state, Some(&timelines), 0.0),
        Vec2::new(14.0, 1.0)
    );
}

#[test]
fn clearing_bone_override_restores_animated_pose() {
    let info = info_with_parent_bone();
    let mut state = AnimationControl::default();
    let timelines = translate_timeline([3.0, 4.0]);
    let bone = info.bone_index("parent").unwrap();
    let animated = child_global_position(&state, Some(&timelines), 0.0);

    state.set_bone_override(
        bone,
        BoneOverride {
            translate: Vec2::new(7.0, 7.0),
        },
    );
    assert_ne!(
        child_global_position(&state, Some(&timelines), 0.0),
        animated
    );
    assert_eq!(
        state.bone_override(bone),
        Some(BoneOverride {
            translate: Vec2::new(7.0, 7.0)
        })
    );

    state.clear_bone_override(bone);
    assert_eq!(state.bone_override(bone), None);
    assert_eq!(
        child_global_position(&state, Some(&timelines), 0.0),
        animated
    );
}

#[test]
fn reports_unknown_bone() {
    assert_eq!(info_with_parent_bone().bone_index("nope"), None);
}

#[test]
fn latest_timeline_value_is_active_at_its_key_time() {
    let timeline =
        Timeline::new([(0.3, AttachmentIndex::INVALID), (0.5, AttachmentIndex(12))].into_iter());

    assert_eq!(timeline.get_latest_value(0.299), None);
    assert_eq!(
        timeline.get_latest_value(0.3),
        Some(AttachmentIndex::INVALID)
    );
    assert_eq!(
        timeline.get_latest_value(0.4),
        Some(AttachmentIndex::INVALID)
    );
    assert_eq!(timeline.get_latest_value(0.5), Some(AttachmentIndex(12)));
}

#[test]
fn timeline_reports_last_key_time() {
    let timeline = Timeline::new([(0.1, "start"), (0.3, "end")].into_iter());

    assert_eq!(timeline.last_time(), Some(0.3));
    assert_eq!(Timeline::<u32>::empty().last_time(), None);
}

#[test]
fn skeleton_info_exposes_duration() {
    let info = SkeletonInfo {
        animation_durations: HashMap::from([("climb_up_r".to_string(), 1.25)]),
        ..default()
    };

    assert!(info.has_animation("climb_up_r"));
    assert_eq!(info.animation_duration("climb_up_r"), Some(1.25));
}

#[test]
fn playback_exposes_exact_seek() {
    let mut playback = AnimationPlayback::default();

    playback.set_current_time_seconds(0.5);
    assert_eq!(playback.current_time_seconds(), 0.5);

    playback.set_current_time_seconds(-1.0);
    assert_eq!(playback.current_time_seconds(), 0.0);
}

#[test]
fn control_setters_do_not_touch_playback_state() {
    let mut control = AnimationControl::default();

    control.set_label("walk");
    assert_eq!(control.label(), "walk");
    assert_eq!(
        control.label_transition,
        DEFAULT_ANIMATION_TRANSITION_DURATION
    );

    control.set_label_with_transition("run", -1.0);
    assert_eq!(control.label_transition, 0.0);

    // Same label: nothing changes
    control.set_label_with_transition("run", 0.5);
    assert_eq!(control.label_transition, 0.0);

    control.set_transition_duration(-2.0);
    assert_eq!(control.transition_duration(), 0.0);
    control.set_time_scale(2.0);
    assert_eq!(control.time_scale(), 2.0);
    control.set_repeat(false);
    assert!(!control.repeat());
}

#[test]
fn playback_is_small() {
    assert!(std::mem::size_of::<AnimationPlayback>() < 128);
}

#[test]
fn skeleton_info_exposes_label_names() {
    let info = SkeletonInfo {
        animation_durations: HashMap::from([
            ("walk_r".to_string(), 0.8),
            ("idle_l".to_string(), 2.0),
        ]),
        ..default()
    };

    let mut labels = info.animation_labels().collect::<Vec<_>>();
    labels.sort_unstable();

    assert_eq!(labels, vec!["idle_l", "walk_r"]);
}

fn event_names(events: impl Iterator<Item = (f32, String)>) -> Vec<String> {
    events.map(|(_, name)| name).collect()
}

#[test]
fn events_do_not_fire_when_time_does_not_advance() {
    let timeline =
        Timeline::new([(0.0, "start".to_string()), (0.5, "mid".to_string())].into_iter());

    assert!(event_names(timeline.iter_range(0.0, 0.0)).is_empty());
    assert!(event_names(timeline.iter_range(0.5, 0.5)).is_empty());

    assert_eq!(event_names(timeline.iter_range(0.4, 0.6)), ["mid"]);
    // Wrapped around the end of the animation
    assert_eq!(event_names(timeline.iter_range(0.9, 0.1)), ["start"]);
}

/// World with a camera and one skeleton type: a single root bone at (0, 5) and two animations
/// "a" and "b" without bone timelines, i.e. with identical poses. "a" has an event "ping" at 0.5.
/// The entity is `AlwaysAnimate`, see [`set_visible`] to make it subject to culling.
fn transition_world() -> (World, Entity) {
    let mut world = World::new();
    world.init_resource::<InstanceInfos>();
    world.init_resource::<Assets<SpineSkeleton>>();
    world.init_resource::<Events<SpineEvent>>();
    world.init_resource::<Time>();
    world.spawn((
        Camera::default(),
        OrthographicProjection::default(),
        GlobalTransform::default(),
    ));

    // "a" has an event at 0.5
    let animation = |events: Timeline<String>| Animation {
        duration: 1.0,
        bone_timelines: HashMap::new(),
        slot_timelines: HashMap::new(),
        events_timeline: events,
    };
    let animations = HashMap::from([
        (
            "a".to_string(),
            animation(Timeline::new([(0.5, "ping".to_string())].into_iter())),
        ),
        ("b".to_string(), animation(Timeline::empty())),
    ]);

    let mut skeleton = SpineSkeleton::with_info_for_tests(SkeletonInfo::default());
    skeleton
        .skeleton
        .add_root_bone(Vec2::new(0.0, 5.0), 0.0, 0.0, Vec2::ONE);
    skeleton.animations = animations;
    let skeleton = world.resource_mut::<Assets<SpineSkeleton>>().add(skeleton);

    let entity = world
        .spawn((
            SkeletonInstance::new(skeleton),
            AnimationControl {
                label: "a".to_string(),
                ..default()
            },
            AnimationPlayback {
                label: "a".to_string(),
                ..default()
            },
            GlobalTransform::default(),
            Aabb::default(),
            AlwaysAnimate,
        ))
        .id();
    world.run_system_once(crate::add_skeleton_instance);
    (world, entity)
}

fn run_frame(world: &mut World, delta: f32) {
    world
        .resource_mut::<Time>()
        .advance_by(std::time::Duration::from_secs_f32(delta));
    world.run_system_once(advance_animations);
}

fn root_bone_translation(world: &mut World, entity: Entity) -> Vec2 {
    world
        .resource_mut::<InstanceInfos>()
        .get_mut(entity)
        .unwrap()
        .bones[0]
        .translation
}

#[test]
fn transition_follows_moving_entity() {
    let (mut world, entity) = transition_world();
    run_frame(&mut world, 0.1);
    assert_eq!(
        root_bone_translation(&mut world, entity),
        Vec2::new(0.0, 5.0)
    );

    world
        .get_mut::<AnimationControl>(entity)
        .unwrap()
        .set_label_with_transition("b", 1.0);
    *world.get_mut::<GlobalTransform>(entity).unwrap() =
        GlobalTransform::from_translation(Vec3::new(100.0, 0.0, 0.0));
    run_frame(&mut world, 0.5);

    // Halfway through the transition between two identical poses: the skeleton must be fully
    // at the new entity position, not halfway between the old and the new one
    assert_eq!(
        root_bone_translation(&mut world, entity),
        Vec2::new(100.0, 5.0)
    );
}

/// Adds a constant root translation animation `label` to the skeleton of `entity`.
fn add_translate_animation(world: &mut World, entity: Entity, label: &str, value: [f32; 2]) {
    let handle = world
        .get::<SkeletonInstance>(entity)
        .unwrap()
        .skeleton
        .clone();
    let mut skeletons = world.resource_mut::<Assets<SpineSkeleton>>();
    let skeleton = skeletons.get_mut(&handle).unwrap();
    skeleton.animations.insert(
        label.to_string(),
        Animation {
            duration: 1.0,
            bone_timelines: HashMap::from([(BoneIndex(0), translate_timeline(value))]),
            slot_timelines: HashMap::new(),
            events_timeline: Timeline::empty(),
        },
    );
}

#[test]
fn transition_started_during_transition_starts_from_displayed_pose() {
    let (mut world, entity) = transition_world();
    add_translate_animation(&mut world, entity, "b", [10.0, 0.0]);
    run_frame(&mut world, 0.1);
    assert_eq!(
        root_bone_translation(&mut world, entity),
        Vec2::new(0.0, 5.0)
    );

    world
        .get_mut::<AnimationControl>(entity)
        .unwrap()
        .set_label_with_transition("b", 1.0);
    run_frame(&mut world, 0.5);
    let displayed = root_bone_translation(&mut world, entity);
    assert!(
        displayed.abs_diff_eq(Vec2::new(5.0, 5.0), 1e-4),
        "{displayed}"
    );

    // Back to "a" (setup pose) while the blend is half way: no jump
    world
        .get_mut::<AnimationControl>(entity)
        .unwrap()
        .set_label_with_transition("a", 1.0);
    run_frame(&mut world, 0.001);
    let after = root_bone_translation(&mut world, entity);
    assert!(after.abs_diff_eq(displayed, 0.05), "{displayed} -> {after}");
}

#[test]
fn advance_animations_does_not_mark_control_changed() {
    let (mut world, entity) = transition_world();
    let count_changed =
        world.register_system(|q: Query<Entity, Changed<AnimationControl>>| q.iter().count());
    // The freshly spawned component counts as changed once
    assert_eq!(world.run_system(count_changed).unwrap(), 1);

    world
        .get_mut::<AnimationControl>(entity)
        .unwrap()
        .set_label("b");
    assert_eq!(world.run_system(count_changed).unwrap(), 1);

    for _ in 0..3 {
        run_frame(&mut world, 0.1);
        assert_eq!(world.run_system(count_changed).unwrap(), 0);
    }
    let playback = world.get::<AnimationPlayback>(entity).unwrap();
    assert_eq!(playback.label(), "b");
    assert!((playback.current_time_seconds() - 0.3).abs() < 1e-4);
}

#[test]
fn label_change_starts_transition_with_label_transition() {
    let (mut world, entity) = transition_world();
    run_frame(&mut world, 0.1);
    assert!(
        !world
            .get::<AnimationPlayback>(entity)
            .unwrap()
            .is_transitioning()
    );

    world
        .get_mut::<AnimationControl>(entity)
        .unwrap()
        .set_label_with_transition("b", 1.0);
    run_frame(&mut world, 0.25);

    let playback = world.get::<AnimationPlayback>(entity).unwrap();
    assert_eq!(playback.label(), "b");
    let transition = playback.transition.as_ref().unwrap();
    assert_eq!(transition.duration, 1.0);
    assert!((transition.elapsed - 0.25).abs() < 1e-5);
    // Source pose only holds the bones of the skeleton
    assert_eq!(transition.source_bones.len(), 1);

    // Runs to completion
    run_frame(&mut world, 1.0);
    assert!(
        !world
            .get::<AnimationPlayback>(entity)
            .unwrap()
            .is_transitioning()
    );
}

#[test]
fn zero_duration_label_change_does_not_transition() {
    let (mut world, entity) = transition_world();
    run_frame(&mut world, 0.1);

    world
        .get_mut::<AnimationControl>(entity)
        .unwrap()
        .set_label_with_transition("b", 0.0);
    run_frame(&mut world, 0.1);

    let playback = world.get::<AnimationPlayback>(entity).unwrap();
    assert_eq!(playback.label(), "b");
    assert!(!playback.is_transitioning());
}

#[test]
fn matching_labels_do_not_transition() {
    let (mut world, entity) = transition_world();
    // Mimic a freshly loaded entity: control and playback agree from the start. Even a large
    // default transition duration must not apply.
    world
        .get_mut::<AnimationControl>(entity)
        .unwrap()
        .label_transition = 5.0;
    run_frame(&mut world, 0.1);

    assert!(
        !world
            .get::<AnimationPlayback>(entity)
            .unwrap()
            .is_transitioning()
    );
}

/// Remove `AlwaysAnimate` from the entity and place its `Aabb` inside (default camera area) or
/// far outside the camera's view.
fn set_visible(world: &mut World, entity: Entity, visible: bool) {
    let center = if visible { 0.0 } else { 1000.0 };
    world
        .entity_mut(entity)
        .remove::<AlwaysAnimate>()
        .insert(Aabb {
            center: Vec3::splat(center).into(),
            half_extents: Vec3::splat(0.5).into(),
        });
}

fn drain_events(world: &mut World) -> Vec<String> {
    world
        .resource_mut::<Events<SpineEvent>>()
        .drain()
        .map(|e| e.name)
        .collect()
}

#[test]
fn visible_entity_is_computed_and_culled_entity_is_not() {
    let (mut world, entity) = transition_world();
    set_visible(&mut world, entity, true);
    run_frame(&mut world, 0.1);
    assert_eq!(
        root_bone_translation(&mut world, entity),
        Vec2::new(0.0, 5.0)
    );

    // Culled: the pose is not recomputed, so it stays where it was
    set_visible(&mut world, entity, false);
    *world.get_mut::<GlobalTransform>(entity).unwrap() =
        GlobalTransform::from_translation(Vec3::new(100.0, 0.0, 0.0));
    run_frame(&mut world, 0.1);
    assert_eq!(
        root_bone_translation(&mut world, entity),
        Vec2::new(0.0, 5.0)
    );

    // Back on screen
    set_visible(&mut world, entity, true);
    run_frame(&mut world, 0.1);
    assert_eq!(
        root_bone_translation(&mut world, entity),
        Vec2::new(100.0, 5.0)
    );
}

#[test]
fn culled_entity_advances_time_and_emits_events() {
    let (mut world, entity) = transition_world();
    set_visible(&mut world, entity, false);

    run_frame(&mut world, 0.4);
    assert!(drain_events(&mut world).is_empty());
    run_frame(&mut world, 0.2);
    assert_eq!(drain_events(&mut world), ["ping"]);

    let playback = world.get::<AnimationPlayback>(entity).unwrap();
    assert!((playback.current_time_seconds() - 0.6).abs() < 1e-5);
}

#[test]
fn culled_entity_progresses_and_finishes_transitions() {
    let (mut world, entity) = transition_world();
    set_visible(&mut world, entity, true);
    run_frame(&mut world, 0.1);

    set_visible(&mut world, entity, false);
    world
        .get_mut::<AnimationControl>(entity)
        .unwrap()
        .set_label_with_transition("b", 1.0);
    run_frame(&mut world, 0.25);

    let playback = world.get::<AnimationPlayback>(entity).unwrap();
    assert_eq!(playback.label(), "b");
    let transition = playback.transition.as_ref().unwrap();
    assert!((transition.elapsed - 0.25).abs() < 1e-5);
    // The source pose is captured when the label changes, whether or not a pose is computed
    assert_eq!(transition.source_bones.len(), 1);

    run_frame(&mut world, 1.0);
    assert!(
        !world
            .get::<AnimationPlayback>(entity)
            .unwrap()
            .is_transitioning()
    );
}

#[test]
fn visible_entity_uses_final_transition_factor_before_dropping_it() {
    let (mut world, entity) = transition_world();
    run_frame(&mut world, 0.1);
    world
        .get_mut::<AnimationControl>(entity)
        .unwrap()
        .set_label_with_transition("b", 0.2);
    *world.get_mut::<GlobalTransform>(entity).unwrap() =
        GlobalTransform::from_translation(Vec3::new(100.0, 0.0, 0.0));
    // Finishes within this frame: the pose is the (fully blended) target pose
    run_frame(&mut world, 0.5);

    assert!(
        !world
            .get::<AnimationPlayback>(entity)
            .unwrap()
            .is_transitioning()
    );
    assert_eq!(
        root_bone_translation(&mut world, entity),
        Vec2::new(100.0, 5.0)
    );
}

#[test]
fn without_camera_playback_advances_and_nothing_is_computed() {
    let (mut world, entity) = transition_world();
    set_visible(&mut world, entity, true);
    run_frame(&mut world, 0.1);

    let cameras: Vec<Entity> = world
        .query_filtered::<Entity, With<Camera>>()
        .iter(&world)
        .collect();
    for camera in cameras {
        world.despawn(camera);
    }

    *world.get_mut::<GlobalTransform>(entity).unwrap() =
        GlobalTransform::from_translation(Vec3::new(100.0, 0.0, 0.0));
    run_frame(&mut world, 0.5);

    assert_eq!(drain_events(&mut world), ["ping"]);
    let playback = world.get::<AnimationPlayback>(entity).unwrap();
    assert!((playback.current_time_seconds() - 0.6).abs() < 1e-5);
    assert_eq!(
        root_bone_translation(&mut world, entity),
        Vec2::new(0.0, 5.0)
    );
}

#[test]
fn always_animate_computes_pose_without_camera() {
    let (mut world, entity) = transition_world();
    let cameras: Vec<Entity> = world
        .query_filtered::<Entity, With<Camera>>()
        .iter(&world)
        .collect();
    for camera in cameras {
        world.despawn(camera);
    }
    *world.get_mut::<GlobalTransform>(entity).unwrap() =
        GlobalTransform::from_translation(Vec3::new(100.0, 0.0, 0.0));
    run_frame(&mut world, 0.1);
    assert_eq!(
        root_bone_translation(&mut world, entity),
        Vec2::new(100.0, 5.0)
    );
}

#[test]
fn setup_pose_matches_animated_pose() {
    let mut world = World::new();
    world.init_resource::<InstanceInfos>();
    world.init_resource::<Assets<SpineSkeleton>>();
    world.init_resource::<Events<SpineEvent>>();
    world.init_resource::<Time>();

    // Rotated, offset root bone with a child, so the multiplication order matters
    let mut skeleton = SpineSkeleton::with_info_for_tests(SkeletonInfo::default());
    let root = skeleton
        .skeleton
        .add_root_bone(Vec2::new(3.0, 5.0), 0.0, 0.5, Vec2::ONE);
    skeleton
        .skeleton
        .add_bone(root, Vec2::new(2.0, 0.0), 0.0, 0.3, Vec2::ONE);
    let skeleton = world.resource_mut::<Assets<SpineSkeleton>>().add(skeleton);

    let mut instance = SkeletonInstance::new(skeleton);
    instance.base_transform =
        Affine2::from_scale_angle_translation(Vec2::new(-1.0, 1.0), 0.0, Vec2::new(1.0, 1.0));
    let entity = world
        .spawn((
            instance,
            AnimationControl::default(),
            AnimationPlayback::default(),
            GlobalTransform::from(
                Transform::from_xyz(100.0, 20.0, 0.0).with_scale(Vec3::new(2.0, 2.0, 1.0)),
            ),
            Aabb::default(),
            AlwaysAnimate,
        ))
        .id();
    world.run_system_once(crate::add_skeleton_instance);
    world.run_system_once(compute_setup_pose);
    let bones = |world: &mut World| {
        world
            .resource_mut::<InstanceInfos>()
            .get_mut(entity)
            .unwrap()
            .bones[..2]
            .to_vec()
    };
    let setup = bones(&mut world);

    run_frame(&mut world, 0.0);
    let animated = bones(&mut world);
    for (setup, animated) in setup.iter().zip(&animated) {
        assert!(
            Affine2::from(*setup).abs_diff_eq(Affine2::from(*animated), 1e-4),
            "setup {setup:?} != animated {animated:?}"
        );
    }
}

fn local(translation: Vec2, rotation: f32, scale: Vec2) -> LocalBoneTransform {
    LocalBoneTransform {
        translation,
        rotation,
        scale,
    }
}

#[test]
fn blend_keeps_rotation_rigid() {
    let from = local(Vec2::ZERO, 0.0, Vec2::ONE);
    let to = local(Vec2::ZERO, PI / 2.0, Vec2::ONE);
    let blended = blend_bone_transform(from, to, 0.5);
    assert!((blended.rotation - PI / 4.0).abs() < 1e-5);
    assert_eq!(blended.scale, Vec2::ONE);

    let matrix = blended.to_affine().matrix2;
    assert!((matrix.x_axis.length() - 1.0).abs() < 1e-5);
    assert!((matrix.y_axis.length() - 1.0).abs() < 1e-5);
    assert!(matrix.x_axis.dot(matrix.y_axis).abs() < 1e-5);
}

#[test]
fn blend_rotates_the_shortest_way() {
    let from = local(Vec2::ZERO, 170f32.to_radians(), Vec2::ONE);
    let to = local(Vec2::ZERO, (-170f32).to_radians(), Vec2::ONE);
    let blended = blend_bone_transform(from, to, 0.5);
    // Half way is +-180 degrees, not 0
    assert!(
        (blended.rotation.abs() - PI).abs() < 1e-4,
        "{}",
        blended.rotation
    );

    // And the other direction
    let blended = blend_bone_transform(to, from, 0.25);
    assert!((blended.rotation - (-175f32).to_radians()).abs() < 1e-4);
}

#[test]
fn blend_flips_mirrored_scale_abruptly() {
    let from = local(Vec2::ZERO, 0.0, Vec2::new(-1.0, 1.0));
    let to = local(Vec2::ZERO, 0.0, Vec2::ONE);
    for factor in [0.25, 0.75] {
        let blended = blend_bone_transform(from, to, factor);
        assert!((blended.scale.x.abs() - 1.0).abs() < 1e-5);
        assert_eq!(blended.scale.y, 1.0);
    }
    assert!(blend_bone_transform(from, to, 0.25).scale.x < 0.0);
    assert!(blend_bone_transform(from, to, 0.5).scale.x > 0.0);
    assert!(blend_bone_transform(from, to, 0.75).scale.x > 0.0);
}

#[test]
fn blend_lerps_translation() {
    let from = local(Vec2::new(0.0, 2.0), 0.0, Vec2::ONE);
    let to = local(Vec2::new(10.0, 4.0), 0.0, Vec2::ONE);
    assert_eq!(
        blend_bone_transform(from, to, 0.5).translation,
        Vec2::new(5.0, 3.0)
    );
}

/// A mirrored root bone (how facing right is usually authored) must mirror the art about the
/// skeleton origin. The base transform offsets that origin from the entity, eg. to the center
/// of a 2-wide footprint, so mirroring about the entity origin would shift the art sideways.
#[test]
fn mirrored_root_bone_mirrors_about_skeleton_origin() {
    let mut world = World::new();
    world.init_resource::<InstanceInfos>();
    world.init_resource::<Assets<SpineSkeleton>>();

    let mut skeleton = SpineSkeleton::with_info_for_tests(SkeletonInfo::default());
    let root = skeleton
        .skeleton
        .add_root_bone(Vec2::ZERO, 0.0, 0.0, Vec2::new(-1.0, 1.0));
    skeleton
        .skeleton
        .add_bone(root, Vec2::new(10.0, 0.0), 0.0, 0.0, Vec2::ONE);
    let skeleton = world.resource_mut::<Assets<SpineSkeleton>>().add(skeleton);

    let mut instance = SkeletonInstance::new(skeleton);
    // Art at half scale, origin offset to the left of the entity
    instance.base_transform =
        Affine2::from_scale_angle_translation(Vec2::splat(0.5), 0.0, Vec2::new(-64.0, 0.0));
    let entity = world
        .spawn((
            instance,
            GlobalTransform::from(Transform::from_xyz(100.0, 20.0, 0.0)),
        ))
        .id();
    world.run_system_once(crate::add_skeleton_instance);
    world.run_system_once(compute_setup_pose);

    let bones = world
        .resource_mut::<InstanceInfos>()
        .get_mut(entity)
        .unwrap()
        .bones[..2]
        .to_vec();
    // The skeleton origin stays where the base transform puts it ...
    assert_eq!(bones[0].translation, Vec2::new(36.0, 20.0));
    // ... and the child is mirrored about it: 100 - 64 - 0.5 * 10
    assert_eq!(bones[1].translation, Vec2::new(31.0, 20.0));
}
