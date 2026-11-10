//! Render-world preparation for the sorted draw group (see `LoadSpineSettings::sorted`).
//!
//! The instances of the sorted skeleton types are drawn in ascending `sort_key` order across all
//! of these types. Every frame:
//! 1. Update the order kept from the last frame (drop despawned, append new instances,
//!    refresh keys and positions) and re-sort it. Only small `SortItem`s are sorted,
//!    never the `InstanceInfo`s themselves.
//! 2. Group the sorted instances into runs of consecutive instances of the same type.
//! 3. Pack the instances in sorted order into the global instance buffers, after the static
//!    types (see `InstanceLayout`). A run is then a consecutive range of instances in these
//!    buffers and is drawn with a single `draw_indexed`.

use std::collections::{HashMap, HashSet};

use bevy::{prelude::*, render::render_asset::RenderAssets};

use crate::{
    SkeletonDrawGroups, SkeletonId,
    render::{
        BONE_SIZE, ExtractedInstances, GpuSpineSkeleton, HEADER_SIZE, TypeInstances, pack_instance,
    },
};

/// Current sort data of an instance, as found in `ExtractedInstances`
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Member {
    pub(crate) key: f32,
    pub(crate) skeleton: SkeletonId,
    /// Instance index within its type in `ExtractedInstances`
    pub(crate) local: u32,
}

/// An entry of the persistent draw order
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SortItem {
    pub(crate) entity: Entity,
    pub(crate) key: f32,
    pub(crate) skeleton: SkeletonId,
    pub(crate) local: u32,
}

/// A run of consecutive instances (in draw order) of the same skeleton type
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Run {
    pub(crate) skeleton: SkeletonId,
    /// Position of the first instance of the run in the sorted draw order, i.e. relative to the
    /// first sorted instance in the instance buffers (`InstanceLayout::sorted_first`)
    pub(crate) first: u32,
    pub(crate) len: u32,
}

#[derive(Resource, Default)]
pub(crate) struct SortedDraw {
    /// Draw order, kept across frames: the previous order is the starting point of the next sort
    order: Vec<SortItem>,
    pub(crate) runs: Vec<Run>,
}

/// Bring the order of the last frame up to date with `members`:
/// - Drop instances that are gone.
/// - Refresh key and buffer position of the remaining ones. The latter changes when instances of
///   the same type despawn, as `InstanceInfos` swap-removes.
/// - Append new instances (in entity order, so that the result is deterministic).
///
/// Does not sort.
pub(crate) fn refresh_order(order: &mut Vec<SortItem>, members: &HashMap<Entity, Member>) {
    order.retain_mut(|item| {
        let Some(m) = members.get(&item.entity) else {
            return false;
        };
        item.key = m.key;
        item.skeleton = m.skeleton;
        item.local = m.local;
        true
    });

    if order.len() == members.len() {
        return;
    }
    let known: HashSet<Entity> = order.iter().map(|item| item.entity).collect();
    let mut new: Vec<_> = members
        .iter()
        .filter(|(entity, _)| !known.contains(entity))
        .map(|(&entity, m)| SortItem {
            entity,
            key: m.key,
            skeleton: m.skeleton,
            local: m.local,
        })
        .collect();
    new.sort_unstable_by_key(|item| item.entity.to_bits());
    order.extend(new);
}

/// Sort by key, ties by entity. The sort is stable and adaptive: for input that is already
/// (nearly) sorted, as in the common case of a few moving instances, it is close to O(n).
pub(crate) fn sort_order(order: &mut [SortItem]) {
    order.sort_by(|a, b| {
        a.key
            .total_cmp(&b.key)
            .then_with(|| a.entity.to_bits().cmp(&b.entity.to_bits()))
    });
}

/// Group the sorted `order` into runs of the same type.
pub(crate) fn build_runs(order: &[SortItem], runs: &mut Vec<Run>) {
    runs.clear();

    for (position, item) in order.iter().enumerate() {
        match runs.last_mut() {
            Some(run) if run.skeleton == item.skeleton => run.len += 1,
            _ => runs.push(Run {
                skeleton: item.skeleton,
                first: position as u32,
                len: 1,
            }),
        }
    }
}

/// Update the draw order of the sorted group and build the runs
pub(super) fn prepare_sorted_draw(
    extracted: Res<ExtractedInstances>,
    draw_groups: Res<SkeletonDrawGroups>,
    gpu_skeletons: Res<RenderAssets<GpuSpineSkeleton>>,
    mut sorted: ResMut<SortedDraw>,
) {
    let mut members = HashMap::new();
    for (&skeleton, t) in &extracted.types {
        if !draw_groups.is_sorted(skeleton) {
            continue;
        }
        if gpu_skeletons
            .get(skeleton)
            .is_none_or(|gpu| gpu.index_count == 0)
        {
            // Skeleton without mesh (or not prepared yet), not drawn
            continue;
        }
        for (local, (&key, &entity)) in t.sort_keys.iter().zip(&t.entities).enumerate() {
            members.insert(
                entity,
                Member {
                    key,
                    skeleton,
                    local: local as u32,
                },
            );
        }
    }

    let SortedDraw { order, runs } = &mut *sorted;
    refresh_order(order, &members);
    sort_order(order);
    build_runs(order, runs);
}

/// Write the sorted instances in draw order to the instance buffers (`headers`, `bones`: their
/// whole staging views). The first sorted instance goes to header `first_header`, its bones to
/// bone `first_bone`; the bones of consecutive instances follow each other.
pub(crate) fn pack_sorted(
    sorted: &SortedDraw,
    types: &HashMap<SkeletonId, TypeInstances>,
    first_header: u32,
    first_bone: u32,
    headers: &mut [u8],
    bones: &mut [u8],
) {
    let mut bone = first_bone as usize;
    for run in &sorted.runs {
        let Some(t) = types.get(&run.skeleton) else {
            continue;
        };
        for position in run.first..run.first + run.len {
            let item = &sorted.order[position as usize];
            if item.local as usize >= t.len() {
                continue;
            }
            let header = (first_header + position) as usize;
            pack_instance(
                t,
                item.local as usize,
                bone as u32,
                &mut headers[header * HEADER_SIZE..(header + 1) * HEADER_SIZE],
                &mut bones[bone * BONE_SIZE..(bone + t.stride) * BONE_SIZE],
            );
            bone += t.stride;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SpineSkeleton;

    fn e(i: u32) -> Entity {
        Entity::from_raw(i)
    }

    /// Skeleton type `n`
    fn ty(n: u32) -> SkeletonId {
        Handle::<SpineSkeleton>::weak_from_u128(n as u128).id()
    }

    fn member(key: f32, skeleton: u32, local: u32) -> Member {
        Member {
            key,
            skeleton: ty(skeleton),
            local,
        }
    }

    fn entities(order: &[SortItem]) -> Vec<u32> {
        order.iter().map(|item| item.entity.index()).collect()
    }

    fn run(skeleton: u32, first: u32, len: u32) -> Run {
        Run {
            skeleton: ty(skeleton),
            first,
            len,
        }
    }

    /// Run `refresh_order` and `sort_order` for one frame
    fn frame(order: &mut Vec<SortItem>, members: &[(Entity, Member)]) {
        let members = members.iter().copied().collect();
        refresh_order(order, &members);
        sort_order(order);
    }

    #[test]
    fn runs_group_consecutive_instances_of_the_same_type() {
        let mut order = Vec::new();
        frame(
            &mut order,
            &[
                (e(1), member(0.0, 5, 0)),
                (e(2), member(1.0, 5, 1)),
                (e(3), member(2.0, 7, 0)),
                (e(4), member(3.0, 5, 2)),
                (e(5), member(4.0, 7, 1)),
                (e(6), member(5.0, 7, 2)),
            ],
        );
        let mut runs = Vec::new();
        build_runs(&order, &mut runs);

        // `first` is the position in the sorted order
        assert_eq!(
            runs,
            vec![run(5, 0, 2), run(7, 2, 1), run(5, 3, 1), run(7, 4, 2)]
        );
    }

    #[test]
    fn runs_of_empty_order_are_empty() {
        let mut runs = vec![run(0, 0, 1)];
        build_runs(&[], &mut runs);
        assert!(runs.is_empty());
    }

    #[test]
    fn higher_key_is_drawn_later_and_ties_are_broken_by_entity() {
        let mut order = Vec::new();
        frame(
            &mut order,
            &[
                (e(9), member(1.0, 0, 0)),
                (e(3), member(2.0, 0, 1)),
                (e(5), member(1.0, 1, 0)),
                (e(4), member(-3.0, 1, 1)),
            ],
        );
        assert_eq!(entities(&order), vec![4, 5, 9, 3]);
    }

    #[test]
    fn order_is_stable_across_frames_with_equal_keys() {
        let members = [
            (e(1), member(0.0, 0, 0)),
            (e(2), member(0.0, 1, 0)),
            (e(3), member(0.0, 0, 1)),
            (e(4), member(0.0, 1, 1)),
        ];
        let mut order = Vec::new();
        frame(&mut order, &members);
        let first = entities(&order);
        assert_eq!(first, vec![1, 2, 3, 4]);

        for _ in 0..5 {
            frame(&mut order, &members);
            assert_eq!(entities(&order), first);
        }
    }

    #[test]
    fn spawn_in_the_middle_of_the_sequence() {
        let mut order = Vec::new();
        frame(
            &mut order,
            &[(e(1), member(0.0, 0, 0)), (e(2), member(10.0, 1, 0))],
        );
        frame(
            &mut order,
            &[
                (e(1), member(0.0, 0, 0)),
                (e(2), member(10.0, 1, 0)),
                (e(3), member(5.0, 0, 1)),
            ],
        );
        assert_eq!(entities(&order), vec![1, 3, 2]);

        let mut runs = Vec::new();
        build_runs(&order, &mut runs);
        assert_eq!(runs, vec![run(0, 0, 2), run(1, 2, 1)]);
    }

    #[test]
    fn despawn_in_the_middle_updates_buffer_positions() {
        // Type 0 holds e1, e2, e3 at positions 0, 1, 2; sorted order e1, e2, e3, e4
        let mut order = Vec::new();
        frame(
            &mut order,
            &[
                (e(1), member(0.0, 0, 0)),
                (e(2), member(1.0, 0, 1)),
                (e(3), member(2.0, 0, 2)),
                (e(4), member(3.0, 1, 0)),
            ],
        );
        assert_eq!(entities(&order), vec![1, 2, 3, 4]);

        // e1 despawns, InstanceInfos swap-removes: e3 moves to position 0
        frame(
            &mut order,
            &[
                (e(2), member(1.0, 0, 1)),
                (e(3), member(2.0, 0, 0)),
                (e(4), member(3.0, 1, 0)),
            ],
        );
        assert_eq!(entities(&order), vec![2, 3, 4]);
        assert_eq!(
            order
                .iter()
                .map(|i| (i.skeleton, i.local))
                .collect::<Vec<_>>(),
            vec![(ty(0), 1), (ty(0), 0), (ty(1), 0)]
        );

        let mut runs = Vec::new();
        build_runs(&order, &mut runs);
        assert_eq!(runs, vec![run(0, 0, 2), run(1, 2, 1)]);
    }

    #[test]
    fn despawn_and_spawn_in_the_same_frame() {
        let mut order = Vec::new();
        frame(
            &mut order,
            &[(e(1), member(0.0, 0, 0)), (e(2), member(1.0, 0, 1))],
        );
        frame(
            &mut order,
            &[(e(2), member(1.0, 0, 0)), (e(7), member(0.5, 0, 1))],
        );
        assert_eq!(entities(&order), vec![7, 2]);
    }

    #[test]
    fn pack_sorted_interleaves_types_with_different_strides() {
        use crate::render::{GlobalBoneTransform, InstanceInfo, InstanceInfos};

        // Type 0 has 1 bone, type 1 has 3. Sorted order: e1 (t0), e3 (t1), e2 (t0), e4 (t1)
        let mut infos = InstanceInfos::default();
        let mut add = |entity: u32, skeleton: u32, bones: usize, x: f32| {
            infos.push(e(entity), ty(skeleton), bones, InstanceInfo::default(), x);
        };
        add(1, 0, 1, 0.0);
        add(2, 0, 1, 2.0);
        add(3, 1, 3, 1.0);
        add(4, 1, 3, 3.0);
        for entity in 1..=4 {
            for (i, b) in infos
                .get_mut(e(entity))
                .unwrap()
                .bones
                .iter_mut()
                .enumerate()
            {
                b.translation = Vec2::new(entity as f32, i as f32);
            }
        }

        let mut sorted = SortedDraw::default();
        let members: HashMap<_, _> = [
            (e(1), member(0.0, 0, 0)),
            (e(2), member(2.0, 0, 1)),
            (e(3), member(1.0, 1, 0)),
            (e(4), member(3.0, 1, 1)),
        ]
        .into();
        refresh_order(&mut sorted.order, &members);
        sort_order(&mut sorted.order);
        build_runs(&sorted.order, &mut sorted.runs);
        assert_eq!(entities(&sorted.order), vec![1, 3, 2, 4]);

        // The sorted section starts at header 5, bone 20
        let (first_header, first_bone) = (5u32, 20u32);
        let mut headers = vec![0u8; (5 + 4) * HEADER_SIZE];
        let mut bones = vec![0u8; (20 + 1 + 3 + 1 + 3) * BONE_SIZE];
        pack_sorted(
            &sorted,
            infos.types(),
            first_header,
            first_bone,
            &mut headers,
            &mut bones,
        );

        let offsets: Vec<u32> = (5..9)
            .map(|h| {
                bytemuck::pod_read_unaligned::<InstanceInfo>(
                    &headers[h * HEADER_SIZE..(h + 1) * HEADER_SIZE],
                )
                .bone_offset
            })
            .collect();
        assert_eq!(offsets, vec![20, 21, 24, 25]);

        let bone = |i: usize| -> Vec2 {
            bytemuck::pod_read_unaligned::<GlobalBoneTransform>(
                &bones[i * BONE_SIZE..(i + 1) * BONE_SIZE],
            )
            .translation
        };
        assert_eq!(bone(20), Vec2::new(1.0, 0.0));
        assert_eq!(bone(21), Vec2::new(3.0, 0.0));
        assert_eq!(bone(23), Vec2::new(3.0, 2.0));
        assert_eq!(bone(24), Vec2::new(2.0, 0.0));
        assert_eq!(bone(25), Vec2::new(4.0, 0.0));
        assert_eq!(bone(27), Vec2::new(4.0, 2.0));
    }
}
