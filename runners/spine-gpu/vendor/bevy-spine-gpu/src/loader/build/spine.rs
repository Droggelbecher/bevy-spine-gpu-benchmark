use std::{collections::HashMap, f32::consts::PI, sync::Arc};

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use bevy::{math::vec2, prelude::*};

use crate::{Atlas, BoneIndex, MAX_ATTACHMENTS_PER_SKELETON, Skeleton, SkeletonInfo, SlotIndex};

use super::{
    SkeletonData,
    mesh::{crop_rectangle_to_region, insert_slot_color, make_mesh},
};
use crate::loader::{
    AttachmentIndex, SlotRule,
    spine_json::{Attachment, DEFAULT_SKIN_NAME, EffectiveSkin, SpineColor, SpineJson, parse},
};

/// Hands out attachment indices, which are assigned sequentially in order of first registration.
#[derive(Default)]
pub(super) struct AttachmentRegistry {
    indices_by_slot: HashMap<SlotIndex, HashMap<String, AttachmentIndex>>,
    count: u32,
}

impl AttachmentRegistry {
    fn register(&mut self, slot: SlotIndex, name: &str) -> AttachmentIndex {
        let count = &mut self.count;
        *self
            .indices_by_slot
            .entry(slot)
            .or_default()
            .entry(name.to_string())
            .or_insert_with(|| {
                *count += 1;
                AttachmentIndex(*count - 1)
            })
    }

    /// Like `register`, but only for attachments the skin (or its fallback) actually has.
    pub(super) fn register_if_in_skin(
        &mut self,
        skin: &EffectiveSkin,
        slot: SlotIndex,
        slot_name: &str,
        name: &str,
    ) -> AttachmentIndex {
        skin.attachment(slot_name, name)
            .map(|_| self.register(slot, name))
            .unwrap_or(AttachmentIndex::INVALID)
    }
}

pub(super) struct SlotInfo {
    bone: BoneIndex,
    /// Setup color of the slot
    color: SpineColor,
    pub(super) index: SlotIndex,
}

/// Builds the [`SkeletonData`] of a skeleton from parsed Spine JSON.
/// The steps must run in order, as each one registers attachment indices:
/// bones, slots (default attachments), skin attachments, animations.
pub(super) struct SpineSkeletonBuilder<'a> {
    /// Path of the .json, for error messages
    path: &'a str,
    spine: &'a SpineJson,
    pub(super) skin: EffectiveSkin<'a>,
    data: SkeletonData,
    pub(super) bones: HashMap<String, BoneIndex>,
    pub(super) slots: HashMap<String, SlotInfo>,
    pub(super) registry: AttachmentRegistry,
}

impl<'a> SpineSkeletonBuilder<'a> {
    /// Starts a builder around the (still empty) `skeleton`.
    fn new(
        path: &'a str,
        spine: &'a SpineJson,
        skin: EffectiveSkin<'a>,
        skeleton: Skeleton,
    ) -> Self {
        Self {
            path,
            spine,
            skin,
            data: SkeletonData::new(skeleton),
            bones: HashMap::new(),
            slots: HashMap::new(),
            registry: AttachmentRegistry::default(),
        }
    }

    /// Looks up a bone by the name used in the JSON.
    pub(super) fn bone(&self, name: &str) -> Result<BoneIndex> {
        self.bones
            .get(name)
            .copied()
            .ok_or_else(|| anyhow!("Bone {:?} not found in {:?}", name, self.path))
    }

    /// Looks up a slot by the name used in the JSON.
    pub(super) fn slot(&self, name: &str) -> Result<&SlotInfo> {
        self.slots
            .get(name)
            .ok_or_else(|| anyhow!("Slot {:?} not found in {:?}", name, self.path))
    }

    /// Adds all bones to the skeleton.
    fn add_bones(&mut self) -> Result<()> {
        for bone in &self.spine.bones {
            let position = vec2(bone.x.unwrap_or_default(), bone.y.unwrap_or_default());
            let length = bone.length.unwrap_or_default();
            let rotation = bone.rotation.unwrap_or_default() * PI / 180.0;
            let scale = vec2(bone.scale_x, bone.scale_y);
            let bone_id = match &bone.parent {
                Some(parent) => {
                    let parent = self.bone(parent)?;
                    self.data
                        .skeleton
                        .add_bone(parent, position, length, rotation, scale)
                }
                None => self
                    .data
                    .skeleton
                    .add_root_bone(position, length, rotation, scale),
            };
            self.bones.insert(bone.name.clone(), bone_id);
        }
        Ok(())
    }

    /// Read slot/bone relationship and register + push the default attachment of each slot.
    fn add_slots(&mut self) -> Result<()> {
        // skin lists / exchange definitions for all attachment names, slots gives the default attachment name
        for (i, slot) in self.spine.slots.iter().enumerate() {
            let index = SlotIndex(i as u32);

            let default_attachment = slot
                .attachment
                .as_ref()
                .map(|a| {
                    self.registry
                        .register_if_in_skin(&self.skin, index, &slot.name, a)
                })
                .unwrap_or(AttachmentIndex::INVALID);

            self.slots.insert(
                slot.name.clone(),
                SlotInfo {
                    bone: self.bone(&slot.bone)?,
                    color: slot.color,
                    index,
                },
            );
            self.data
                .skeleton
                .default_attachments
                .push(default_attachment);
            trace!(
                "[slot {}] name={:?} default_attach={}",
                i, slot.name, default_attachment.0
            );
        }
        Ok(())
    }

    /// Creates the mesh of each skin attachment, extending the registry with non-default ones.
    fn add_attachments(&mut self, slot_rules: &[SlotRule]) -> Result<()> {
        for (slot_name, attachment_name, attachment_skin, attachment) in self.skin.attachments() {
            let Some(slot) = self.slots.get(&slot_name) else {
                bail!(
                    "Couldnt find slot named {:?} in {:?}. Available slots: {}",
                    slot_name,
                    self.path,
                    self.slots
                        .keys()
                        .map(|name| format!("{name:?}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            };

            let (attachment, region_name) = attachment
                .resolve(self.spine, &slot_name, &attachment_name, &mut Vec::new())
                .context(format!(
                    "while resolving skin {:?}, slot {:?}, attachment {:?}",
                    attachment_skin.name, slot_name, attachment_name
                ))?;
            let attachment_index = self.registry.register(slot.index, &attachment_name);

            let mesh = build_attachment_mesh(
                &self.data.skeleton.atlas,
                attachment,
                &region_name,
                slot,
                attachment_index,
            )
            .context(format!("slot {:?} attach {:?}", slot_name, attachment_name))?;

            // Spine slot order is draw order, so the slot index doubles as z
            let phase = slot_rules
                .iter()
                .find(|rule| slot_name.starts_with(&rule.prefix))
                .map_or(0, |rule| rule.phase);
            self.data.add_mesh(mesh, phase, slot.index.0, self.path)?;
        }
        Ok(())
    }

    /// Creates the timelines of every animation.
    fn add_animations(&mut self) -> Result<()> {
        for (name, spine_animation) in &self.spine.animations {
            let animation = self.load_animation(spine_animation)?;
            self.data.animations.insert(name.clone(), animation);
        }
        Ok(())
    }

    /// Validates the registry and returns the skeleton data with its name lookups.
    fn finish(mut self) -> Result<SkeletonData> {
        // All attachments (including ones only referenced from animation timelines) are registered now
        check_attachment_count(self.path, self.registry.count as usize)?;

        for (slot_idx, attachments) in self.registry.indices_by_slot.iter() {
            for (attachment_name, attachment_index) in attachments {
                trace!(
                    "[attach {}] slot {} attachment {:?}",
                    attachment_index.0, slot_idx.0, attachment_name
                );
            }
        }

        self.data.info = SkeletonInfo {
            slot_indices_by_name: self
                .slots
                .iter()
                .map(|(n, s)| (n.clone(), s.index))
                .collect(),
            bone_indices_by_name: self.bones,
            attachment_indices_by_slot: self.registry.indices_by_slot,
            ..default()
        };
        Ok(self.data)
    }
}

/// Build the mesh of one (already resolved) skin attachment, including the slot color attribute.
/// `region_name` is the atlas region the attachment is drawn from.
fn build_attachment_mesh(
    atlas: &Atlas,
    mut attachment: Attachment,
    region_name: &str,
    slot: &SlotInfo,
    attachment_index: AttachmentIndex,
) -> Result<Mesh> {
    let region = atlas
        .get(region_name)
        .with_context(|| format!("atlas region {:?} not found", region_name))?;
    let spine_uv_transform = atlas.get_spine_uv_transform(region_name).unwrap();
    let region_uv_transform = atlas.get_region_uv_transform(region_name).unwrap();

    crop_rectangle_to_region(&mut attachment, &region);

    let mut mesh = make_mesh(
        &attachment,
        slot.bone,
        spine_uv_transform,
        region_uv_transform,
        attachment_index,
    )?;
    // Spine: slot color x attachment color (the skeleton color is the instance color)
    let [sr, sg, sb, sa] = slot.color.to_f32();
    let [ar, ag, ab, aa] = attachment.color().to_f32();
    let color = [sr * ar, sg * ag, sb * ab, sa * aa];
    insert_slot_color(&mut mesh, color);
    Ok(mesh)
}

/// The parsed contents of a Spine `.json`, ready to be built into a skeleton for any of its
/// skins.
pub(crate) struct SpineFile {
    /// Path of the .json, for error messages
    path: String,
    spine: SpineJson,
    slot_rules: Arc<[SlotRule]>,
}

/// Parse the contents of the `.json` at `path`. `path` is only used in error messages.
pub(crate) fn parse_spine_file(path: &str, json: &str) -> Result<SpineFile> {
    let spine = parse(json).with_context(|| format!("While parsing {:?}", path))?;
    if spine.has_color_timelines() {
        warn!("{path:?}: animated slot colors are not supported, the slot setup color is used");
    }
    Ok(SpineFile {
        path: path.to_string(),
        spine,
        slot_rules: Arc::new([]),
    })
}

impl SpineFile {
    /// Assign slots to draw phases by name (see [`SlotRule`]).
    pub(crate) fn with_slot_rules(mut self, rules: Arc<[SlotRule]>) -> Self {
        self.slot_rules = rules;
        self
    }

    /// Names of the skins of the file.
    pub(crate) fn skin_names(&self) -> impl Iterator<Item = &str> {
        self.spine.skins.iter().map(|skin| skin.name.as_str())
    }

    /// Build the skeleton with the given skin (`None`: the file's only or default skin) and
    /// atlas (with the texture set).
    pub(crate) fn build(&self, selected_skin: Option<&str>, atlas: Atlas) -> Result<SkeletonData> {
        let Self {
            path,
            spine,
            slot_rules,
        } = self;
        let skin = EffectiveSkin {
            skin: spine.select_skin(selected_skin, path)?,
            default_skin: spine.find_skin(DEFAULT_SKIN_NAME),
        };

        let skeleton = Skeleton::new(
            atlas,
            vec2(spine.skeleton.width, spine.skeleton.height),
            vec2(spine.skeleton.x, spine.skeleton.y),
        );
        let mut builder = SpineSkeletonBuilder::new(path, spine, skin, skeleton);
        builder.add_bones()?;
        builder.add_slots()?;
        builder.add_attachments(slot_rules)?;
        builder.add_animations()?;
        builder.finish()
    }
}

/// Error if a skeleton has more attachments than fit into the GPU visibility mask.
fn check_attachment_count(path: &str, count: usize) -> Result<()> {
    ensure!(
        count <= MAX_ATTACHMENTS_PER_SKELETON,
        "Too many attachments for skeleton {:?}: {} (max {})",
        path,
        count,
        MAX_ATTACHMENTS_PER_SKELETON
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachment_limit_is_inclusive() {
        assert!(check_attachment_count("ok", MAX_ATTACHMENTS_PER_SKELETON).is_ok());
    }

    #[test]
    fn attachment_limit_exceeded_errors() {
        let err = check_attachment_count("big", MAX_ATTACHMENTS_PER_SKELETON + 1).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Too many attachments for skeleton \"big\": 97 (max 96)"
        );
    }
}
