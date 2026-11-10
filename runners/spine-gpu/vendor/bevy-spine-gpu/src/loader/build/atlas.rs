use std::collections::HashMap;

use anyhow::{Context as _, Result};
use bevy::{
    math::{DVec2, vec2},
    prelude::*,
};

use crate::{
    Animation, Atlas, BoneIndex, Skeleton, SkeletonInfo, SlotIndex, Timeline, atlas::Region,
};

use super::{SkeletonData, mesh::make_mesh};
use crate::loader::{AttachmentIndex, spine_json::Attachment};

/// The bounding box of the regions of an atlas as `make_atlas_skeleton` places them (centered at
/// their offset), as `(bottom left, size)`. Empty for an atlas without regions.
fn region_bounds(atlas: &Atlas) -> (Vec2, Vec2) {
    let (min, max) = atlas.iter_regions().fold(
        (Vec2::INFINITY, Vec2::NEG_INFINITY),
        |(min, max),
         (
            _,
            _,
            Region {
                min: rmin,
                max: rmax,
                offset,
            },
        )| {
            let half = (rmax - rmin) / 2.0;
            (min.min(offset - half), max.max(offset + half))
        },
    );
    if min.cmple(max).all() {
        (min, max - min)
    } else {
        (Vec2::ZERO, Vec2::ZERO)
    }
}

/// The name of the animation of an atlas region: the region name without an optional `prefix-`
/// (word characters followed by a dash) and without an optional trailing `_<digits>` frame index.
fn animation_name(region_name: &str) -> &str {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let without_prefix = match region_name.split_once('-') {
        Some((prefix, rest)) if !prefix.is_empty() && prefix.chars().all(is_word) => rest,
        _ => region_name,
    };
    let without_digits = without_prefix.trim_end_matches(|c: char| c.is_ascii_digit());
    without_digits.strip_suffix('_').unwrap_or(without_prefix)
}

/// Create a skeleton from an atlas (with its texture set); `path` is only used in error messages.
/// Each region is one static animation. The bounds of the skeleton are the union of the regions.
pub(crate) fn make_atlas_skeleton(path: &str, atlas: Atlas) -> Result<SkeletonData> {
    let (aabb_bottom_left, size) = region_bounds(&atlas);
    let mut data = SkeletonData::new(Skeleton::new(atlas, size, aabb_bottom_left));
    data.skeleton
        .add_root_bone(vec2(0.0, 0.0), 0.0, 0.0, Vec2::ONE);
    data.skeleton.default_attachments.push(AttachmentIndex(0));

    let slot = SlotIndex(0);
    let slot_indices_by_name = HashMap::from([("image".to_string(), slot)]);
    let mut attachment_indices_by_slot = HashMap::from([(slot, HashMap::new())]);
    let mut animations = HashMap::new();

    // Collected up front as the loop below mutates the skeleton
    let regions = data.skeleton.atlas.iter_regions().collect::<Vec<_>>();
    for (
        region_index,
        name,
        Region {
            min, max, offset, ..
        },
    ) in regions
    {
        let sz = max - min;
        // Note: For the special case of a pure atlas skeleton, attachment index and region index are defined to be the same thing and there is only one slot.
        let attachment_index = AttachmentIndex(region_index.0);

        trace!(
            "Atlas [{}] min={:?} max={:?} offs={:?}",
            name, min, max, offset
        );

        let region_uv_transform = data
            .skeleton
            .atlas
            .get_region_uv_transform(&name)
            .with_context(|| format!("No UV transform for atlas region {:?}", name))?;
        let mesh = make_mesh(
            &Attachment::new_rectangle(offset.x, offset.y, sz.x, sz.y),
            BoneIndex(0),
            (DVec2::ONE, DVec2::ZERO),
            region_uv_transform,
            attachment_index,
        )?;
        data.add_mesh(mesh, 0, 0, path)?;

        let anim_name = animation_name(&name).to_string();
        attachment_indices_by_slot
            .entry(slot)
            .or_default()
            .insert(name.clone(), attachment_index);
        attachment_indices_by_slot
            .entry(slot)
            .or_default()
            .insert(anim_name.clone(), attachment_index);

        let mut slot_timelines = HashMap::new();
        slot_timelines.insert(
            slot,
            Timeline {
                entries: vec![(0.0, attachment_index)],
            },
        );
        animations.insert(
            anim_name,
            Animation {
                duration: 1.0,
                bone_timelines: default(),
                slot_timelines,
                events_timeline: Timeline::empty(),
            },
        );
    }

    trace!("animations {:?}", animations);

    data.info = SkeletonInfo {
        slot_indices_by_name,
        attachment_indices_by_slot,
        ..default()
    };
    data.animations = animations;
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn animation_name_strips_prefix_and_frame_index() {
        assert_eq!(animation_name("walk"), "walk");
        assert_eq!(animation_name("walk_2"), "walk");
        assert_eq!(animation_name("walk_"), "walk");
        assert_eq!(animation_name("plant-walk_12"), "walk");
        assert_eq!(animation_name("plant-walk"), "walk");
        assert_eq!(animation_name("a_1_2"), "a_1");
        // Only trailing digits after an underscore are a frame index
        assert_eq!(animation_name("walk2"), "walk2");
        assert_eq!(animation_name("two words-x"), "two words-x");
    }

    #[test]
    fn region_bounds_is_the_union_of_the_regions() {
        let mut atlas = Atlas::new();
        // 10 x 20 centered at (0, 10) and 4 x 4 centered at (10, 2)
        atlas.add_rect("a", Vec2::ZERO, vec2(10.0, 20.0), vec2(0.0, 10.0));
        atlas.add_rect("b", vec2(20.0, 20.0), vec2(24.0, 24.0), vec2(10.0, 2.0));

        assert_eq!(region_bounds(&atlas), (vec2(-5.0, 0.0), vec2(17.0, 20.0)));
    }

    #[test]
    fn region_bounds_of_an_empty_atlas_is_empty() {
        assert_eq!(region_bounds(&Atlas::new()), (Vec2::ZERO, Vec2::ZERO));
    }
}
