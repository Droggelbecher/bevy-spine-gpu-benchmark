use std::{
    cmp::Reverse,
    collections::{BTreeMap, HashMap},
    ops::Range,
};

use bevy::{
    math::{Affine2, vec2},
    prelude::*,
    render::primitives::Aabb,
};

use crate::{
    Animation, Atlas, AttachmentIndex, AttachmentOverrideError, LoadSpineSettings, SlotIndex,
    SpineGPUConfig, VertexInfo, animation::entity_affine,
};

pub const DEFAULT_RENDER_BUCKET: i32 = 0;

/// Identifies a skeleton type, keys the per-type render data
pub(crate) type SkeletonId = AssetId<SpineSkeleton>;

/// Which draw group a skeleton type belongs to. A property of the type, not of its file: set once
/// when the type is first requested (see [`SkeletonDrawGroups`]) and never rewritten.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SkeletonDrawGroup {
    /// Drawn in the given render bucket, one draw per type. Lower buckets are drawn first; higher
    /// buckets are drawn later and therefore appear on top.
    Bucket(i32),
    /// Drawn after all other (static) types, with instances ordered across skeleton types by
    /// `SkeletonInstance::sort_key`.
    Sorted,
}

impl SkeletonDrawGroup {
    pub(crate) fn from_settings(settings: Option<&LoadSpineSettings>) -> Self {
        match settings {
            Some(s) if s.sorted => Self::Sorted,
            Some(s) => Self::Bucket(s.render_bucket),
            None => Self::Bucket(DEFAULT_RENDER_BUCKET),
        }
    }

    pub(crate) fn is_sorted(self) -> bool {
        self == Self::Sorted
    }

    /// Render bucket for static types. Sorted types are not drawn by bucket, so they report the default.
    pub(crate) fn render_bucket(self) -> i32 {
        match self {
            Self::Bucket(b) => b,
            Self::Sorted => DEFAULT_RENDER_BUCKET,
        }
    }
}

/// The draw group of every requested skeleton type.
///
/// Main world resource, extracted to the render world when changed. The first request of a type
/// decides its group, later conflicting requests are ignored (with a warning by the caller).
#[derive(Resource, Clone, Default, Debug)]
pub(crate) struct SkeletonDrawGroups {
    /// Type -> group and registration sequence number, which breaks ties between types of the same
    /// bucket (the later registered type is drawn first)
    groups: HashMap<AssetId<SpineSkeleton>, (SkeletonDrawGroup, u32)>,
    /// The static types in effective draw order. Sorted types are drawn according to `SortedDraw`.
    draw_order: Vec<AssetId<SpineSkeleton>>,
}

impl SkeletonDrawGroups {
    /// Register `skeleton` with `group` unless it is already registered. Returns the group the
    /// type uses.
    pub(crate) fn register(
        &mut self,
        skeleton: AssetId<SpineSkeleton>,
        group: SkeletonDrawGroup,
    ) -> SkeletonDrawGroup {
        if let Some(&(existing, _)) = self.groups.get(&skeleton) {
            return existing;
        }
        let sequence = self.groups.len() as u32;
        self.groups.insert(skeleton, (group, sequence));
        self.update_draw_order();
        group
    }

    /// The group of `skeleton`, if registered
    pub(crate) fn group(&self, skeleton: AssetId<SpineSkeleton>) -> Option<SkeletonDrawGroup> {
        self.groups.get(&skeleton).map(|&(group, _)| group)
    }

    pub(crate) fn is_sorted(&self, skeleton: AssetId<SpineSkeleton>) -> bool {
        self.groups
            .get(&skeleton)
            .is_some_and(|(group, _)| group.is_sorted())
    }

    /// The static types in effective draw order: lower buckets first.
    pub(crate) fn draw_order(&self) -> &[AssetId<SpineSkeleton>] {
        &self.draw_order
    }

    fn update_draw_order(&mut self) {
        self.draw_order = self
            .groups
            .iter()
            .filter(|(_, (group, _))| !group.is_sorted())
            .map(|(&skeleton, _)| skeleton)
            .collect();
        self.draw_order.sort_by_key(|skeleton| {
            let (group, sequence) = self.groups[skeleton];
            (group.render_bucket(), Reverse(sequence))
        });
    }
}

/// A skeleton type, shared by all [`SkeletonInstance`]s of it: bones, mesh topology, the atlas
/// texture, animations and name lookups. Loaded from a Spine `.json` (with the
/// `.atlas` next to it) or an `.atlas` file, or built from an image.
///
/// Read-only for application code, see [`SkeletonInfo`] via [`SpineSkeleton::info`].
#[derive(Asset, TypePath, Clone, Debug)]
pub struct SpineSkeleton {
    pub(crate) skeleton: Skeleton,
    /// Vertex data of the meshes, in the order their topology was added to `skeleton`
    pub(crate) vertices: Vec<VertexInfo>,
    pub(crate) animations: HashMap<String, Animation>,
    pub(crate) info: SkeletonInfo,
}

impl SpineSkeleton {
    /// Name lookups and animation durations of this skeleton type.
    pub fn info(&self) -> &SkeletonInfo {
        &self.info
    }

    pub(crate) fn texture(&self) -> Handle<Image> {
        self.skeleton.atlas.get_texture()
    }

    /// A skeleton without any geometry or animation that only knows `info`, so code driving
    /// slots, bones or animations by name can be tested without loading a skeleton.
    #[cfg(test)]
    pub fn with_info_for_tests(info: SkeletonInfo) -> Self {
        Self {
            skeleton: Skeleton::new(Atlas::new(), Vec2::ONE, Vec2::ZERO),
            vertices: Vec::new(),
            animations: HashMap::new(),
            info,
        }
    }
}

/// Type of Skeleton
/// This is free roaming, do NOT put this on your animation
#[derive(Clone, Debug)]
pub(crate) struct Skeleton {
    pub(crate) bones: Vec<Bone>,

    // All the triangles of all attachments of this skeleton
    // [(draw phase, slot index / zlayer)] -> [vertex index]
    pub(crate) meshes: BTreeMap<(usize, u32), Vec<u32>>,
    next_index: u32,

    pub(crate) atlas: Atlas,
    // [slot_index]
    pub(crate) default_attachments: Vec<AttachmentIndex>,

    pub(crate) size: Vec2,
    pub(crate) aabb_bottom_left: Vec2,
}

/// Read-only name lookups of a skeleton type: slot / bone / attachment names to the opaque indices
/// used by [`AnimationControl`](crate::AnimationControl), and the loaded animations with their
/// durations.
///
/// Part of the [`SpineSkeleton`] shared by all instances, reached via
/// [`SkeletonInstance::info`].
#[derive(Clone, Debug, Default)]
pub struct SkeletonInfo {
    // slot name -> slot index
    pub(crate) slot_indices_by_name: HashMap<String, SlotIndex>,
    // bone name -> bone index (empty for single image / atlas skeletons)
    pub(crate) bone_indices_by_name: HashMap<String, BoneIndex>,
    // slot index -> attachment name -> attachment index
    // attachment names are "relative" to the slot
    pub(crate) attachment_indices_by_slot: HashMap<SlotIndex, HashMap<String, AttachmentIndex>>,
    // animation label -> duration in seconds
    pub(crate) animation_durations: HashMap<String, f32>,
}

/// Put this on your animation to select a skeleton
#[derive(Component, Clone, Debug)]
pub struct SkeletonInstance {
    pub(crate) skeleton: Handle<SpineSkeleton>,
    pub(crate) color: Vec4,
    pub(crate) base_transform: Affine2,
    /// Draw order key, only used for skeleton types loaded with `LoadSpineSettings::sorted`:
    /// instances of those types are drawn in ascending `sort_key` order across all such types,
    /// so higher keys end up in front. Ties are broken by entity.
    pub sort_key: f32,
}

#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct BoneIndex(pub(crate) u32);

#[derive(Debug, Clone, Copy)]
pub struct Bone {
    pub(crate) parent: BoneIndex,
    pub(crate) setup_rotation: f32, // radians
    pub(crate) setup_position: Vec2,
    pub(crate) setup_scale: Vec2,
}

impl BoneIndex {
    pub const INVALID: BoneIndex = BoneIndex(u32::MAX);
}

impl SkeletonInfo {
    /// Return all animation labels loaded for this skeleton.
    pub fn animation_labels(&self) -> impl Iterator<Item = &str> {
        self.animation_durations.keys().map(String::as_str)
    }

    /// Resolve a Spine slot name to the opaque index used by runtime animation data.
    pub fn slot_index(&self, slot_name: &str) -> Option<SlotIndex> {
        self.slot_indices_by_name.get(slot_name).copied()
    }

    /// Resolve an attachment name within a slot.
    pub fn attachment_index(
        &self,
        slot: SlotIndex,
        attachment_name: &str,
    ) -> Option<AttachmentIndex> {
        self.attachment_indices_by_slot
            .get(&slot)?
            .get(attachment_name)
            .copied()
    }

    /// Resolve slot and attachment names together.
    pub fn slot_attachment_indices(
        &self,
        slot_name: &str,
        attachment_name: &str,
    ) -> Result<(SlotIndex, AttachmentIndex), AttachmentOverrideError> {
        let slot =
            self.slot_index(slot_name)
                .ok_or_else(|| AttachmentOverrideError::SlotNotFound {
                    slot: slot_name.to_string(),
                })?;

        let attachment = self
            .attachment_index(slot, attachment_name)
            .ok_or_else(|| AttachmentOverrideError::AttachmentNotFound {
                slot: slot_name.to_string(),
                attachment: attachment_name.to_string(),
            })?;

        Ok((slot, attachment))
    }

    /// Resolve a Spine bone name to the opaque index used by runtime animation data.
    pub fn bone_index(&self, bone_name: &str) -> Option<BoneIndex> {
        self.bone_indices_by_name.get(bone_name).copied()
    }

    pub fn animation_duration(&self, label: &str) -> Option<f32> {
        self.animation_durations.get(label).copied()
    }

    pub fn has_animation(&self, label: &str) -> bool {
        self.animation_durations.contains_key(label)
    }

    /// An empty info that only knows the given slot and bone names, so code driving slots and
    /// bones by name can be tested without loading a skeleton.
    #[cfg(test)]
    pub fn with_names_for_tests(slot_names: &[&str], bone_names: &[&str]) -> Self {
        Self {
            slot_indices_by_name: slot_names
                .iter()
                .enumerate()
                .map(|(i, n)| (n.to_string(), SlotIndex(i as u32)))
                .collect(),
            bone_indices_by_name: bone_names
                .iter()
                .enumerate()
                .map(|(i, n)| (n.to_string(), BoneIndex(i as u32)))
                .collect(),
            ..Default::default()
        }
    }
}

impl SkeletonInstance {
    pub fn new(skeleton: Handle<SpineSkeleton>) -> Self {
        Self {
            skeleton,
            color: Vec4::ONE,
            base_transform: Affine2::IDENTITY,
            sort_key: 0.0,
        }
    }

    /// The skeleton type shared by all instances of it.
    pub fn skeleton(&self) -> &Handle<SpineSkeleton> {
        &self.skeleton
    }

    /// The [`SkeletonInfo`] of this instance's skeleton type, if the skeleton is (still) loaded.
    pub fn info<'a>(&self, skeletons: &'a Assets<SpineSkeleton>) -> Option<&'a SkeletonInfo> {
        skeletons.get(&self.skeleton).map(SpineSkeleton::info)
    }

    pub fn color(&self) -> Vec4 {
        self.color
    }

    pub fn set_color(&mut self, color: Vec4) {
        self.color = color;
    }

    pub fn base_transform(&self) -> Affine2 {
        self.base_transform
    }

    pub fn set_base_transform(&mut self, base_transform: Affine2) {
        self.base_transform = base_transform;
    }
}

impl Skeleton {
    pub fn new(atlas: Atlas, size: Vec2, aabb_bottom_left: Vec2) -> Self {
        Self {
            bones: Vec::new(),
            meshes: default(),
            atlas,
            default_attachments: default(),
            next_index: 0u32,
            size,
            aabb_bottom_left,
        }
    }

    pub fn add_root_bone(
        &mut self,
        position: Vec2,
        _length: f32,
        rotation: f32,
        scale: Vec2,
    ) -> BoneIndex {
        self.add_bone(BoneIndex::INVALID, position, _length, rotation, scale)
    }

    /// rotation: radians
    pub fn add_bone(
        &mut self,
        parent: BoneIndex,
        position: Vec2,
        _length: f32,
        rotation: f32,
        scale: Vec2,
    ) -> BoneIndex {
        if parent == BoneIndex::INVALID {
            assert!(self.bones.is_empty(), "root bone must be the first bone");
        }

        self.bones.push(Bone {
            parent,
            setup_rotation: rotation,
            setup_position: position,
            setup_scale: scale,
        });
        BoneIndex(self.bones.len() as u32 - 1)
    }

    /// After adding a mesh to VertexConfigurations, insert it here so the topology is known
    pub fn add_mesh(&mut self, mesh: &Mesh, phase: usize, z_index: u32) {
        let mesh: Vec<u32> = mesh
            .indices()
            .unwrap()
            .iter()
            .map(|i| i as u32 + self.next_index)
            .collect();
        self.next_index = mesh.iter().max().unwrap() + 1u32;
        self.meshes
            .entry((phase, z_index))
            .or_default()
            .extend(mesh);
    }

    /// The index buffer contents: all meshes grouped by draw phase, in z order within each
    /// phase, and the range of each phase in it (indexed by phase, empty for phases without
    /// meshes, missing after the last phase with meshes).
    pub(crate) fn phase_indices(&self) -> (Vec<u32>, Vec<Range<u32>>) {
        let mut indices = Vec::new();
        let mut ranges: Vec<Range<u32>> = Vec::new();
        for (&(phase, _z), mesh) in &self.meshes {
            let start = indices.len() as u32;
            if ranges.len() <= phase {
                ranges.resize(phase + 1, start..start);
            }
            indices.extend_from_slice(mesh);
            ranges[phase].end = indices.len() as u32;
        }
        (indices, ranges)
    }
}

/// Keep each instance's [`Aabb`] up to date.
///
/// Unlike Bevy's own use of [`Aabb`], this one is in world space: the skeleton's bounds, padded
/// by `SpineGPUPlugin::aabb_padding` so animated poses still fit, transformed by the entity
/// transform and [`SkeletonInstance::base_transform`]. `advance_animations` culls against it.
pub(crate) fn update_aabb(
    query: Query<
        (Entity, &SkeletonInstance, &GlobalTransform),
        Or<(Changed<GlobalTransform>, Changed<SkeletonInstance>)>,
    >,
    mut commands: Commands,
    skeletons: Res<Assets<SpineSkeleton>>,
    config: Res<SpineGPUConfig>,
) {
    for (entity, skeleton_ref, gt) in &query {
        let Some(skeleton) = skeletons.get(&skeleton_ref.skeleton) else {
            continue;
        };
        let (min, max) = world_bounds(
            &skeleton.skeleton,
            config.aabb_padding,
            entity_affine(gt) * skeleton_ref.base_transform,
        );
        commands
            .entity(entity)
            .insert(Aabb::from_min_max(min.extend(0.0), max.extend(0.0)));
    }
}

/// Padded bounds of `skeleton` in skeleton space, transformed by `transform`.
fn world_bounds(skeleton: &Skeleton, padding: f32, transform: Affine2) -> (Vec2, Vec2) {
    // Pad a full longest skeleton size to each side, so the box is not too small even for the
    // animated skeleton
    let pad = Vec2::splat(skeleton.size.max_element() * padding);
    let min = skeleton.aabb_bottom_left - pad;
    let max = skeleton.aabb_bottom_left + skeleton.size + pad;

    [min, vec2(min.x, max.y), vec2(max.x, min.y), max]
        .map(|corner| transform.transform_point2(corner))
        .into_iter()
        .fold((Vec2::INFINITY, Vec2::NEG_INFINITY), |(min, max), p| {
            (min.min(p), max.max(p))
        })
}

#[cfg(test)]
mod draw_group_tests {
    use super::*;

    fn id(n: u128) -> AssetId<SpineSkeleton> {
        Handle::<SpineSkeleton>::weak_from_u128(n).id()
    }

    #[test]
    fn draw_order_sorts_by_bucket_then_reverse_registration() {
        let mut groups = SkeletonDrawGroups::default();
        for (n, bucket) in [0, 2, 0, -1].into_iter().enumerate() {
            groups.register(id(n as u128), SkeletonDrawGroup::Bucket(bucket));
        }
        assert_eq!(groups.draw_order(), [id(3), id(2), id(0), id(1)]);
    }

    #[test]
    fn draw_order_skips_sorted_types() {
        let mut groups = SkeletonDrawGroups::default();
        groups.register(id(0), SkeletonDrawGroup::Bucket(0));
        groups.register(id(1), SkeletonDrawGroup::Sorted);
        groups.register(id(2), SkeletonDrawGroup::Bucket(0));
        assert_eq!(groups.draw_order(), [id(2), id(0)]);
        assert!(groups.is_sorted(id(1)));
        assert!(!groups.is_sorted(id(0)));
    }

    #[test]
    fn first_registration_wins() {
        let mut groups = SkeletonDrawGroups::default();
        groups.register(id(0), SkeletonDrawGroup::Bucket(3));
        let group = groups.register(id(0), SkeletonDrawGroup::Sorted);
        assert_eq!(group, SkeletonDrawGroup::Bucket(3));
        assert!(!groups.is_sorted(id(0)));
        assert_eq!(groups.draw_order(), [id(0)]);
    }

    #[test]
    fn draw_group_from_settings() {
        assert_eq!(
            SkeletonDrawGroup::from_settings(None),
            SkeletonDrawGroup::Bucket(DEFAULT_RENDER_BUCKET)
        );
        let mut settings = LoadSpineSettings {
            render_bucket: 7,
            ..default()
        };
        assert_eq!(
            SkeletonDrawGroup::from_settings(Some(&settings)),
            SkeletonDrawGroup::Bucket(7)
        );
        settings.sorted = true;
        assert_eq!(
            SkeletonDrawGroup::from_settings(Some(&settings)),
            SkeletonDrawGroup::Sorted
        );
    }
}

#[cfg(test)]
mod aabb_tests {
    use super::*;

    fn skeleton() -> Skeleton {
        // 10 x 20, bottom center at the origin
        Skeleton::new(Atlas::new(), vec2(10.0, 20.0), vec2(-5.0, 0.0))
    }

    #[test]
    fn bounds_follow_translation() {
        let bounds = world_bounds(
            &skeleton(),
            0.0,
            Affine2::from_translation(vec2(100.0, 50.0)),
        );
        assert_eq!(bounds, (vec2(95.0, 50.0), vec2(105.0, 70.0)));
    }

    #[test]
    fn bounds_follow_scale_and_flip() {
        // Flipped vertically and scaled by 2: the box extends below the entity
        let bounds = world_bounds(&skeleton(), 0.0, Affine2::from_scale(vec2(2.0, -2.0)));
        assert_eq!(bounds, (vec2(-10.0, -40.0), vec2(10.0, 0.0)));
    }

    #[test]
    fn bounds_include_scaled_padding() {
        // Padding is the longest side (20) times the factor, scaled with the skeleton
        let bounds = world_bounds(&skeleton(), 0.5, Affine2::from_scale(Vec2::splat(2.0)));
        assert_eq!(bounds, (vec2(-30.0, -20.0), vec2(30.0, 60.0)));
    }
}

#[cfg(test)]
mod phase_tests {
    use bevy::render::{mesh::Indices, render_asset::RenderAssetUsages};

    use super::*;

    /// A mesh with `n` vertices and the indices `0..n`
    fn mesh(n: u32) -> Mesh {
        Mesh::new(
            bevy::render::mesh::PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, vec![Vec3::ZERO; n as usize])
        .with_inserted_indices(Indices::U32((0..n).collect()))
    }

    #[test]
    fn indices_are_grouped_by_phase_then_z() {
        let mut skeleton = Skeleton::new(Atlas::new(), Vec2::ONE, Vec2::ZERO);
        skeleton.add_mesh(&mesh(3), 0, 1); // vertices 0..3
        skeleton.add_mesh(&mesh(3), 2, 0); // 3..6
        skeleton.add_mesh(&mesh(3), 0, 0); // 6..9

        let (indices, ranges) = skeleton.phase_indices();
        assert_eq!(indices, [6, 7, 8, 0, 1, 2, 3, 4, 5]);
        // Phase 1 has no meshes: an empty range at the phase boundary
        assert_eq!(ranges, [0..6, 6..6, 6..9]);
    }

    #[test]
    fn no_meshes_no_ranges() {
        let skeleton = Skeleton::new(Atlas::new(), Vec2::ONE, Vec2::ZERO);
        assert_eq!(skeleton.phase_indices(), (vec![], vec![]));
    }
}
