use std::collections::HashMap;

use anyhow::Result;
use bevy::{
    math::{DVec2, vec2},
    prelude::*,
};

use crate::{Atlas, BoneIndex, Skeleton, SkeletonInfo, SlotIndex};

use super::{SkeletonData, mesh::make_mesh};
use crate::loader::{AttachmentIndex, spine_json::Attachment};

pub(crate) fn make_image_skeleton(texture: Handle<Image>, size: UVec2) -> Result<SkeletonData> {
    let size = size.as_vec2();
    let mut atlas = Atlas::new();
    atlas.set_size(size);
    atlas.set_texture(texture);
    atlas.add_rect("image", Vec2::ZERO, size, Vec2::ZERO);

    let aabb_bottom_left = vec2(-size.x / 2.0, 0.0);
    let mut data = SkeletonData::new(Skeleton::new(atlas, size, aabb_bottom_left));
    data.skeleton
        .add_root_bone(vec2(0.0, 0.0), 0.0, 0.0, Vec2::ONE);
    data.skeleton.default_attachments.push(AttachmentIndex(0));

    let mesh = make_mesh(
        &Attachment::new_rectangle(0.0, size.y / 2.0, size.x, size.y),
        BoneIndex(0),
        (DVec2::ONE, DVec2::ZERO),
        (DVec2::ONE, DVec2::ZERO),
        AttachmentIndex(0),
    )?;
    data.add_mesh(mesh, 0, 0, "image")?;

    let slot = SlotIndex(0);
    let attachment = AttachmentIndex(0);
    data.info = SkeletonInfo {
        slot_indices_by_name: HashMap::from([("image".to_string(), slot)]),
        attachment_indices_by_slot: HashMap::from([(
            slot,
            HashMap::from([("image".to_string(), attachment)]),
        )]),
        ..default()
    };
    Ok(data)
}
