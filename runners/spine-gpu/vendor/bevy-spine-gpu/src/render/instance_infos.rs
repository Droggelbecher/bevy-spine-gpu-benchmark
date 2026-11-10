use std::{collections::HashMap, num::NonZeroU64, ops::Mul};

use bevy::{
    math::Affine2,
    prelude::*,
    render::{
        Extract,
        render_resource::{
            BindGroup, BindGroupEntry, Buffer, BufferDescriptor, BufferUsages, ShaderType,
        },
        renderer::{RenderDevice, RenderQueue},
    },
};
use bytemuck::{Pod, Zeroable, cast_slice};

use crate::{
    SkeletonDrawGroups, SkeletonId, SkeletonInstance, SpineSkeleton,
    render::{
        SpineGPUPipeline,
        draw_order::{Run, SortedDraw, pack_sorted},
    },
};

// Attachment visibility is a bitmask stored in a `vec3<u32>` (`InstanceInfo::attachments`).
// Keep in sync with `uvec3_get_bit` in spine_gpu.wgsl
pub const MAX_ATTACHMENTS_PER_SKELETON: usize = 3 * 32;

mod shader_types {
    #![allow(dead_code)] // encase 0.8.0 ShaderType derive emits unused `check` fns.

    use super::*;

    /// The header of an instance, `instance_infos[instance_index]` in the shader. The bones of the
    /// instance are in a separate buffer, starting at `bone_offset`.
    /// Keep in sync with `InstanceInfo` in spine_gpu.wgsl
    #[derive(Clone, Copy, Zeroable, Debug, ShaderType, Pod)]
    #[repr(C)]
    pub(crate) struct InstanceInfo {
        pub(crate) attachments: UVec3,
        /// Index (in bones, not bytes) of the first bone of this instance in the bone buffer.
        /// Set when packing the buffers every frame, the value in the main world is meaningless.
        pub(crate) bone_offset: u32,
        /// Multiplicative tint of the instance
        pub(crate) color: Vec4,
    }

    #[derive(Clone, Copy, Default, Zeroable, Debug, ShaderType, Pod)]
    #[repr(C)]
    pub(crate) struct GlobalBoneTransform {
        pub(crate) matrix2: Mat2,
        pub(crate) translation: Vec2,
        pub(crate) _pad: Vec2,
    }
}

pub(crate) use shader_types::{GlobalBoneTransform, InstanceInfo};

// The sizes of the buffer elements, which the WGSL structs `InstanceInfo` and `BoneTransform` in
// spine_gpu.wgsl must match: `vec3<u32>` + `u32` + `vec4<f32>` (align 16), and `mat2x2<f32>` +
// `vec2<f32>` + padding.
const _: () = assert!(size_of::<InstanceInfo>() == 32);
const _: () = assert!(size_of::<GlobalBoneTransform>() == 32);

pub(crate) const HEADER_SIZE: usize = size_of::<InstanceInfo>();
pub(crate) const BONE_SIZE: usize = size_of::<GlobalBoneTransform>();
const BONE_OFFSET_FIELD: usize = std::mem::offset_of!(InstanceInfo, bone_offset);

impl Default for InstanceInfo {
    fn default() -> Self {
        Self {
            attachments: Default::default(),
            bone_offset: 0,
            // Multiplicative tint; white leaves the rendered colors unchanged
            color: Vec4::ONE,
        }
    }
}

/// The instances of one skeleton type, instance-major: instance `i` has the header `headers[i]`
/// and the bones `bones[i * stride..(i + 1) * stride]`. All per-instance vectors are indexed by
/// the same instance index.
#[derive(Default, Debug)]
pub(crate) struct TypeInstances {
    /// Number of bones of every instance of this type (at least 1)
    pub(crate) stride: usize,
    pub(crate) headers: Vec<InstanceInfo>,
    pub(crate) bones: Vec<GlobalBoneTransform>,
    /// `SkeletonInstance::sort_key`
    pub(crate) sort_keys: Vec<f32>,
    pub(crate) entities: Vec<Entity>,
}

impl Clone for TypeInstances {
    fn clone(&self) -> Self {
        let mut c = Self::default();
        c.clone_from(self);
        c
    }

    /// Reuses the allocations of `self`
    fn clone_from(&mut self, source: &Self) {
        self.stride = source.stride;
        self.headers.clone_from(&source.headers);
        self.bones.clone_from(&source.bones);
        self.sort_keys.clone_from(&source.sort_keys);
        self.entities.clone_from(&source.entities);
    }
}

impl TypeInstances {
    fn new(stride: usize) -> Self {
        Self {
            stride,
            ..default()
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.headers.len()
    }

    pub(crate) fn bones_of(&self, idx: usize) -> &[GlobalBoneTransform] {
        &self.bones[idx * self.stride..(idx + 1) * self.stride]
    }
}

/// The instances of all skeleton types. Only types with at least one instance have entries.
#[derive(Resource, Default)]
pub(crate) struct InstanceInfos {
    types: HashMap<SkeletonId, TypeInstances>,
    // [main world entity] => (skeleton type, instance index)
    position_by_entity: HashMap<Entity, (SkeletonId, usize)>,
}

/// The data of one instance in `InstanceInfos`
pub(crate) struct InstanceMut<'a> {
    pub(crate) header: &'a mut InstanceInfo,
    /// One transform per bone of the skeleton type
    pub(crate) bones: &'a mut [GlobalBoneTransform],
}

impl InstanceInfos {
    pub(crate) fn types(&self) -> &HashMap<SkeletonId, TypeInstances> {
        &self.types
    }

    pub(crate) fn get_mut(&mut self, entity: Entity) -> Option<InstanceMut<'_>> {
        let &(skeleton, idx) = self.position_by_entity.get(&entity)?;
        let t = self.types.get_mut(&skeleton)?;
        let stride = t.stride;
        Some(InstanceMut {
            header: &mut t.headers[idx],
            bones: &mut t.bones[idx * stride..(idx + 1) * stride],
        })
    }

    /// Append an instance of the type `skeleton` that has `n_bones` bones
    pub(crate) fn push(
        &mut self,
        entity: Entity,
        skeleton: SkeletonId,
        n_bones: usize,
        header: InstanceInfo,
        sort_key: f32,
    ) {
        debug_assert!(!self.position_by_entity.contains_key(&entity));
        let stride = n_bones.max(1);
        let t = self
            .types
            .entry(skeleton)
            .or_insert_with(|| TypeInstances::new(stride));
        debug_assert_eq!(t.stride, stride, "bone count of a skeleton type changed");

        let idx = t.headers.len();
        t.headers.push(header);
        t.bones
            .resize(t.bones.len() + t.stride, GlobalBoneTransform::default());
        t.sort_keys.push(sort_key);
        t.entities.push(entity);
        self.position_by_entity.insert(entity, (skeleton, idx));
    }

    /// Remove the instance of `entity` by moving the last instance of its type (header, bones, key
    /// and entity) into its place.
    pub(crate) fn remove(&mut self, entity: Entity) {
        let Some((skeleton, idx)) = self.position_by_entity.remove(&entity) else {
            return;
        };
        let Some(t) = self.types.get_mut(&skeleton) else {
            return;
        };

        let last = t.headers.len() - 1;
        let stride = t.stride;
        if idx != last {
            t.bones
                .copy_within(last * stride..(last + 1) * stride, idx * stride);
        }
        t.bones.truncate(last * stride);
        t.headers.swap_remove(idx);
        t.sort_keys.swap_remove(idx);
        t.entities.swap_remove(idx);

        if idx != last {
            // The former last instance now lives at idx
            self.position_by_entity
                .insert(t.entities[idx], (skeleton, idx));
        }

        if t.headers.is_empty() {
            self.types.remove(&skeleton);
        }
    }
}

impl From<Affine2> for GlobalBoneTransform {
    fn from(value: Affine2) -> Self {
        Self {
            matrix2: value.matrix2,
            translation: value.translation,
            _pad: Vec2::ZERO,
        }
    }
}

impl From<GlobalBoneTransform> for Affine2 {
    fn from(value: GlobalBoneTransform) -> Self {
        Affine2::from_mat2_translation(value.matrix2, value.translation)
    }
}

impl Mul<Affine2> for GlobalBoneTransform {
    type Output = Self;

    fn mul(self, rhs: Affine2) -> Self::Output {
        (Affine2::from(self) * rhs).into()
    }
}

/// (main world)
/// For a newly created skeleton instance, create the entry in `instance_infos`
pub(crate) fn add_skeleton_instance(
    query: Query<(Entity, &SkeletonInstance), Added<SkeletonInstance>>,
    mut instance_infos: ResMut<InstanceInfos>,
    skeletons: Res<Assets<SpineSkeleton>>,
) {
    for (entity, skeleton_ref) in &query {
        // Instances only get their `SkeletonInstance` once the skeleton is loaded
        let Some(skeleton) = skeletons.get(&skeleton_ref.skeleton) else {
            warn!("New skeleton instance {entity:?} has no loaded skeleton, not adding it");
            continue;
        };

        instance_infos.push(
            entity,
            skeleton_ref.skeleton.id(),
            skeleton.skeleton.bones.len(),
            InstanceInfo {
                color: skeleton_ref.color,
                ..default()
            },
            skeleton_ref.sort_key,
        );
    }
}

/// (main world) Copy changed sort keys to `instance_infos`
pub(crate) fn update_sort_keys(
    query: Query<(Entity, &SkeletonInstance), Changed<SkeletonInstance>>,
    mut instance_infos: ResMut<InstanceInfos>,
) {
    for (entity, skeleton_ref) in &query {
        let Some(&(skeleton, idx)) = instance_infos.position_by_entity.get(&entity) else {
            continue;
        };
        if let Some(key) = instance_infos
            .types
            .get_mut(&skeleton)
            .and_then(|t| t.sort_keys.get_mut(idx))
        {
            *key = skeleton_ref.sort_key;
        }
    }
}

pub(super) fn remove_skeleton_instance(
    mut removed: RemovedComponents<SkeletonInstance>,
    mut instance_infos: ResMut<InstanceInfos>,
) {
    for removed_entity in removed.read() {
        instance_infos.remove(removed_entity);
    }
}

// --------------------------------------------------------------------------------
// Extraction
// --------------------------------------------------------------------------------

/// (render world) The instance data of the main world's `InstanceInfos`, without its entity map.
/// Persistent: updated in place every frame, reusing the allocations.
#[derive(Resource, Default)]
pub(crate) struct ExtractedInstances {
    pub(crate) types: HashMap<SkeletonId, TypeInstances>,
}

impl ExtractedInstances {
    fn sync_from(&mut self, source: &HashMap<SkeletonId, TypeInstances>) {
        self.types
            .retain(|skeleton, _| source.contains_key(skeleton));
        for (skeleton, instances) in source {
            match self.types.get_mut(skeleton) {
                Some(extracted) => extracted.clone_from(instances),
                None => {
                    self.types.insert(*skeleton, instances.clone());
                }
            }
        }
    }
}

pub(super) fn extract_instance_infos(
    instance_infos: Extract<Res<InstanceInfos>>,
    mut extracted: ResMut<ExtractedInstances>,
) {
    extracted.sync_from(&instance_infos.types);
}

// --------------------------------------------------------------------------------
// Layout and packing
// --------------------------------------------------------------------------------

/// Where the draws find their instances in the global header buffer: the static types in draw
/// order come first, then the sorted instances in draw order. The instance index of a draw is the
/// index of a header, as `instance_index` of the shader includes `first_instance`.
#[derive(Resource, Default, Debug, PartialEq)]
pub(crate) struct InstanceLayout {
    pub(crate) static_draws: Vec<StaticDraw>,
    /// Index of the header of the first sorted instance. `Run::first` is relative to this.
    pub(crate) sorted_first: u32,
    /// Index of the first bone of the first sorted instance
    sorted_first_bone: u32,
    total_headers: u32,
    total_bones: u32,
}

/// The instances of a static type
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StaticDraw {
    pub(crate) skeleton: SkeletonId,
    /// Index of the first header
    pub(crate) first: u32,
    pub(crate) len: u32,
    first_bone: u32,
}

impl InstanceLayout {
    /// `static_order`: the static types in draw order. `runs`: the sorted draw.
    pub(crate) fn compute(
        &mut self,
        static_order: &[SkeletonId],
        types: &HashMap<SkeletonId, TypeInstances>,
        runs: &[Run],
    ) {
        self.static_draws.clear();
        let (mut headers, mut bones) = (0u32, 0u32);
        for &skeleton in static_order {
            let Some(t) = types.get(&skeleton) else {
                continue;
            };
            let len = t.len() as u32;
            self.static_draws.push(StaticDraw {
                skeleton,
                first: headers,
                len,
                first_bone: bones,
            });
            headers += len;
            bones += len * t.stride as u32;
        }
        self.sorted_first = headers;
        self.sorted_first_bone = bones;
        for run in runs {
            headers += run.len;
            bones += run.len * types.get(&run.skeleton).map_or(0, |t| t.stride as u32);
        }
        self.total_headers = headers;
        self.total_bones = bones;
    }
}

/// Write all instances of `t` to the staging slices `headers_out` and `bones_out`, which are
/// exactly as large as the instances. The first bone is at `first_bone` in the bone buffer.
fn pack_type(t: &TypeInstances, first_bone: u32, headers_out: &mut [u8], bones_out: &mut [u8]) {
    headers_out.copy_from_slice(cast_slice(&t.headers));
    bones_out.copy_from_slice(cast_slice(&t.bones));
    for i in 0..t.len() {
        let offset = first_bone + (i * t.stride) as u32;
        let at = i * HEADER_SIZE + BONE_OFFSET_FIELD;
        headers_out[at..at + 4].copy_from_slice(&offset.to_ne_bytes());
    }
}

/// Write instance `idx` of `t` to `header_out` and `bones_out`, which are exactly as large as it.
pub(crate) fn pack_instance(
    t: &TypeInstances,
    idx: usize,
    bone_offset: u32,
    header_out: &mut [u8],
    bones_out: &mut [u8],
) {
    header_out.copy_from_slice(bytemuck::bytes_of(&t.headers[idx]));
    header_out[BONE_OFFSET_FIELD..BONE_OFFSET_FIELD + 4]
        .copy_from_slice(&bone_offset.to_ne_bytes());
    bones_out.copy_from_slice(cast_slice(t.bones_of(idx)));
}

// --------------------------------------------------------------------------------
// Buffers
// --------------------------------------------------------------------------------

/// Elements a buffer holds at least
const MIN_CAPACITY: u64 = 64;

/// The new capacity (in elements) of a buffer that holds `capacity` and has to hold `needed`
/// elements, or `None` if it keeps its size: it grows to the next power of two when too small and
/// shrinks only when less than a quarter is used.
fn new_capacity(capacity: u64, needed: u64) -> Option<u64> {
    if needed > capacity || capacity == 0 || (capacity > MIN_CAPACITY && needed < capacity / 4) {
        Some(needed.next_power_of_two().max(MIN_CAPACITY))
    } else {
        None
    }
}

/// The global header and bone buffers of all instances, and the bind group of both (group 1)
#[derive(Resource, Default)]
pub(crate) struct InstanceInfosBuffers {
    /// Buffer and its capacity in elements
    headers: Option<(Buffer, u64)>,
    bones: Option<(Buffer, u64)>,
    pub(crate) bind_group: Option<BindGroup>,
}

impl InstanceInfosBuffers {
    /// Make the buffers hold `headers` and `bones` elements. The bind group is only recreated when
    /// a buffer is. The buffers are never empty (wgpu rejects empty bindings).
    fn ensure_capacity(
        &mut self,
        device: &RenderDevice,
        pipeline: &SpineGPUPipeline,
        headers: u64,
        bones: u64,
    ) {
        let mut changed = self.bind_group.is_none();
        for (slot, needed, element_size, label) in [
            (&mut self.headers, headers, HEADER_SIZE, "InstanceHeaders"),
            (&mut self.bones, bones, BONE_SIZE, "InstanceBones"),
        ] {
            let capacity = slot.as_ref().map_or(0, |(_, capacity)| *capacity);
            let Some(capacity) = new_capacity(capacity, needed) else {
                continue;
            };
            changed = true;
            *slot = Some((
                device.create_buffer(&BufferDescriptor {
                    label: Some(label),
                    size: capacity * element_size as u64,
                    usage: BufferUsages::COPY_DST | BufferUsages::STORAGE,
                    mapped_at_creation: false,
                }),
                capacity,
            ));
        }

        if changed && let (Some((headers, _)), Some((bones, _))) = (&self.headers, &self.bones) {
            self.bind_group = Some(device.create_bind_group(
                "InstanceInfosBindGroup",
                &pipeline.instance_infos_layout,
                &[
                    BindGroupEntry {
                        binding: 0,
                        resource: headers.as_entire_binding(),
                    },
                    BindGroupEntry {
                        binding: 1,
                        resource: bones.as_entire_binding(),
                    },
                ],
            ));
        }
    }
}

/// Pack all instances in draw order into the global buffers: the static types, then the sorted
/// group (see [`InstanceLayout`]). Hidden instances are written too, their data is just stale.
#[allow(clippy::too_many_arguments)]
pub(super) fn prepare_instance_infos(
    extracted: Res<ExtractedInstances>,
    draw_groups: Res<SkeletonDrawGroups>,
    sorted: Res<SortedDraw>,
    mut layout: ResMut<InstanceLayout>,
    mut buffers: ResMut<InstanceInfosBuffers>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    pipeline: Res<SpineGPUPipeline>,
) {
    layout.compute(draw_groups.draw_order(), &extracted.types, &sorted.runs);
    buffers.ensure_capacity(
        &render_device,
        &pipeline,
        layout.total_headers as u64,
        layout.total_bones as u64,
    );

    let (Some(headers_size), Some(bones_size)) = (
        NonZeroU64::new(layout.total_headers as u64 * HEADER_SIZE as u64),
        NonZeroU64::new(layout.total_bones as u64 * BONE_SIZE as u64),
    ) else {
        // No instances, nothing is drawn
        return;
    };
    let (Some((header_buffer, _)), Some((bone_buffer, _))) = (&buffers.headers, &buffers.bones)
    else {
        return;
    };
    let (Some(mut headers_view), Some(mut bones_view)) = (
        render_queue.write_buffer_with(header_buffer, 0, headers_size),
        render_queue.write_buffer_with(bone_buffer, 0, bones_size),
    ) else {
        return;
    };

    for draw in &layout.static_draws {
        let t = &extracted.types[&draw.skeleton];
        let first_header = draw.first as usize;
        let first_bone = draw.first_bone as usize;
        pack_type(
            t,
            draw.first_bone,
            &mut headers_view[first_header * HEADER_SIZE..(first_header + t.len()) * HEADER_SIZE],
            &mut bones_view[first_bone * BONE_SIZE..(first_bone + t.len() * t.stride) * BONE_SIZE],
        );
    }

    pack_sorted(
        &sorted,
        &extracted.types,
        layout.sorted_first,
        layout.sorted_first_bone,
        &mut headers_view,
        &mut bones_view,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(i: u32) -> Entity {
        Entity::from_raw(i)
    }

    fn ty(n: u32) -> SkeletonId {
        Handle::<SpineSkeleton>::weak_from_u128(n as u128).id()
    }

    fn header(tag: f32) -> InstanceInfo {
        InstanceInfo {
            color: Vec4::splat(tag),
            ..default()
        }
    }

    fn tag(h: &InstanceInfo) -> f32 {
        h.color.x
    }

    /// Mark the bones of an instance: x = `tag`, y = bone index
    fn fill_bones(infos: &mut InstanceInfos, entity: Entity, tag: f32) {
        for (i, b) in infos.get_mut(entity).unwrap().bones.iter_mut().enumerate() {
            b.translation = Vec2::new(tag, i as f32);
        }
    }

    fn bone_marks(infos: &InstanceInfos, skeleton: u32) -> Vec<Vec<(f32, f32)>> {
        let t = &infos.types()[&ty(skeleton)];
        (0..t.len())
            .map(|i| {
                t.bones_of(i)
                    .iter()
                    .map(|b| (b.translation.x, b.translation.y))
                    .collect()
            })
            .collect()
    }

    fn add(infos: &mut InstanceInfos, entity: u32, skeleton: u32, bones: usize, tag: f32) {
        infos.push(e(entity), ty(skeleton), bones, header(tag), tag);
        fill_bones(infos, e(entity), tag);
    }

    fn run(skeleton: u32, first: u32, len: u32) -> Run {
        Run {
            skeleton: ty(skeleton),
            first,
            len,
        }
    }

    #[test]
    fn header_matches_wgsl_layout() {
        assert_eq!(HEADER_SIZE, 32);
        assert_eq!(BONE_OFFSET_FIELD, 12);
        assert_eq!(BONE_SIZE, 32);
        // The sizes WGSL gives the structs, which are the strides of the storage buffers
        assert_eq!(InstanceInfo::min_size().get() as usize, HEADER_SIZE);
        assert_eq!(GlobalBoneTransform::min_size().get() as usize, BONE_SIZE);
    }

    #[test]
    fn push_sizes_bone_blocks_by_stride() {
        let mut infos = InstanceInfos::default();
        add(&mut infos, 1, 0, 3, 1.0);
        add(&mut infos, 2, 0, 3, 2.0);
        add(&mut infos, 3, 1, 0, 3.0); // no bones: still one slot
        assert_eq!(infos.types()[&ty(0)].bones.len(), 6);
        assert_eq!(infos.types()[&ty(1)].bones.len(), 1);
        assert_eq!(infos.get_mut(e(2)).unwrap().bones.len(), 3);
        assert_eq!(infos.get_mut(e(3)).unwrap().bones.len(), 1);
    }

    #[test]
    fn swap_remove_moves_header_bone_block_key_and_entity() {
        let mut infos = InstanceInfos::default();
        for i in 1..=4 {
            add(&mut infos, i, 0, 3, i as f32);
        }
        // Another type with a different stride must be untouched
        add(&mut infos, 10, 1, 2, 10.0);
        add(&mut infos, 11, 1, 2, 11.0);

        infos.remove(e(2));

        let t = &infos.types()[&ty(0)];
        // e4 moved into the place of e2
        assert_eq!(t.entities, vec![e(1), e(4), e(3)]);
        assert_eq!(
            t.headers.iter().map(tag).collect::<Vec<_>>(),
            [1.0, 4.0, 3.0]
        );
        assert_eq!(t.sort_keys, vec![1.0, 4.0, 3.0]);
        assert_eq!(
            bone_marks(&infos, 0),
            vec![
                vec![(1.0, 0.0), (1.0, 1.0), (1.0, 2.0)],
                vec![(4.0, 0.0), (4.0, 1.0), (4.0, 2.0)],
                vec![(3.0, 0.0), (3.0, 1.0), (3.0, 2.0)],
            ]
        );
        // Entity lookups follow the move
        assert_eq!(tag(infos.get_mut(e(4)).unwrap().header), 4.0);
        assert_eq!(infos.get_mut(e(4)).unwrap().bones[1].translation.x, 4.0);
        assert!(infos.get_mut(e(2)).is_none());
        assert_eq!(
            bone_marks(&infos, 1),
            vec![
                vec![(10.0, 0.0), (10.0, 1.0)],
                vec![(11.0, 0.0), (11.0, 1.0)]
            ]
        );
    }

    #[test]
    fn remove_last_and_only_instance() {
        let mut infos = InstanceInfos::default();
        add(&mut infos, 1, 0, 2, 1.0);
        add(&mut infos, 2, 0, 2, 2.0);
        infos.remove(e(2));
        assert_eq!(infos.types()[&ty(0)].entities, vec![e(1)]);
        assert_eq!(infos.types()[&ty(0)].bones.len(), 2);
        infos.remove(e(1));
        assert!(infos.types().is_empty());
        // Removing an unknown entity is fine
        infos.remove(e(1));
        // The type can come back
        add(&mut infos, 3, 0, 5, 3.0);
        assert_eq!(infos.get_mut(e(3)).unwrap().bones.len(), 5);
    }

    #[test]
    fn extraction_follows_the_main_world() {
        let mut infos = InstanceInfos::default();
        add(&mut infos, 1, 0, 3, 1.0);
        add(&mut infos, 2, 1, 2, 2.0);

        let mut extracted = ExtractedInstances::default();
        extracted.sync_from(&infos.types);
        assert_eq!(extracted.types[&ty(0)].bones.len(), 3);
        assert_eq!(extracted.types[&ty(1)].entities, vec![e(2)]);

        infos.remove(e(2));
        add(&mut infos, 3, 0, 3, 3.0);
        extracted.sync_from(&infos.types);
        assert!(!extracted.types.contains_key(&ty(1)));
        assert_eq!(extracted.types[&ty(0)].entities, vec![e(1), e(3)]);
        assert_eq!(extracted.types[&ty(0)].bones.len(), 6);
    }

    #[test]
    fn layout_places_static_types_then_sorted_runs() {
        let mut infos = InstanceInfos::default();
        add(&mut infos, 1, 0, 3, 1.0);
        add(&mut infos, 2, 0, 3, 2.0);
        add(&mut infos, 3, 1, 1, 3.0);
        add(&mut infos, 4, 2, 4, 4.0); // sorted
        add(&mut infos, 5, 2, 4, 5.0); // sorted
        add(&mut infos, 6, 3, 2, 6.0); // sorted

        let runs = [run(2, 0, 1), run(3, 1, 1), run(2, 2, 1)];
        let mut layout = InstanceLayout::default();
        // Draw order: type 1 before type 0; type 9 has no instances
        layout.compute(&[ty(1), ty(0), ty(9)], infos.types(), &runs);

        assert_eq!(
            layout.static_draws,
            vec![
                StaticDraw {
                    skeleton: ty(1),
                    first: 0,
                    len: 1,
                    first_bone: 0
                },
                StaticDraw {
                    skeleton: ty(0),
                    first: 1,
                    len: 2,
                    first_bone: 1
                },
            ]
        );
        assert_eq!(layout.sorted_first, 3);
        assert_eq!(layout.sorted_first_bone, 7);
        assert_eq!(layout.total_headers, 6);
        assert_eq!(layout.total_bones, 7 + 4 + 2 + 4);
    }

    #[test]
    fn pack_type_sets_bone_offsets_by_stride() {
        let mut infos = InstanceInfos::default();
        add(&mut infos, 1, 0, 3, 1.0);
        add(&mut infos, 2, 0, 3, 2.0);
        let t = &infos.types()[&ty(0)];

        let mut headers = vec![0u8; 2 * HEADER_SIZE];
        let mut bones = vec![0u8; 6 * BONE_SIZE];
        pack_type(t, 10, &mut headers, &mut bones);

        let headers: Vec<InstanceInfo> = headers
            .chunks(HEADER_SIZE)
            .map(bytemuck::pod_read_unaligned)
            .collect();
        assert_eq!(headers[0].bone_offset, 10);
        assert_eq!(headers[1].bone_offset, 13);
        assert_eq!(tag(&headers[1]), 2.0);
        let bone: GlobalBoneTransform =
            bytemuck::pod_read_unaligned(&bones[3 * BONE_SIZE..4 * BONE_SIZE]);
        assert_eq!(bone.translation, Vec2::new(2.0, 0.0));
    }

    #[test]
    fn pack_instance_writes_one_block() {
        let mut infos = InstanceInfos::default();
        add(&mut infos, 1, 0, 3, 1.0);
        add(&mut infos, 2, 0, 3, 2.0);
        let t = &infos.types()[&ty(0)];

        let mut header = vec![0u8; HEADER_SIZE];
        let mut bones = vec![0u8; 3 * BONE_SIZE];
        pack_instance(t, 1, 42, &mut header, &mut bones);

        let header: InstanceInfo = bytemuck::pod_read_unaligned(&header);
        assert_eq!(header.bone_offset, 42);
        assert_eq!(tag(&header), 2.0);
        let bone: GlobalBoneTransform = bytemuck::pod_read_unaligned(&bones[2 * BONE_SIZE..]);
        assert_eq!(bone.translation, Vec2::new(2.0, 2.0));
    }

    #[test]
    fn capacity_grows_to_powers_of_two_and_shrinks_below_a_quarter() {
        // Initial
        assert_eq!(new_capacity(0, 0), Some(MIN_CAPACITY));
        assert_eq!(new_capacity(0, 100), Some(128));
        // Fits: unchanged
        assert_eq!(new_capacity(128, 100), None);
        assert_eq!(new_capacity(128, 33), None);
        assert_eq!(new_capacity(MIN_CAPACITY, 0), None);
        // Grow
        assert_eq!(new_capacity(128, 129), Some(256));
        // Shrink only below a quarter
        assert_eq!(new_capacity(1024, 256), None);
        assert_eq!(new_capacity(1024, 255), Some(256));
        assert_eq!(new_capacity(1024, 0), Some(MIN_CAPACITY));
    }
}
