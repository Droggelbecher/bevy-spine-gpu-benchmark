use std::collections::HashMap;

use anyhow::Result;
use bevy::log::warn;

use crate::{
    Animation, BoneIndex, BoneTimelines, CurveDef, CurveTimeline, KeyDef, SlotIndex, Timeline,
};

use super::spine::SpineSkeletonBuilder;
use crate::loader::{
    AttachmentIndex,
    spine_json::{
        self, Curve, RotateTimelineEntry, ScaleTimelineEntry, SlotTimelines, TranslateTimelineEntry,
    },
};

/// Duration of an animation: the time of its last key over all timelines.
fn animation_duration(
    bone_timelines: &HashMap<BoneIndex, BoneTimelines>,
    slot_timelines: &HashMap<SlotIndex, Timeline<AttachmentIndex>>,
    events_timeline: &Timeline<String>,
) -> f32 {
    let bone_times = bone_timelines.values().flat_map(|t| {
        [
            t.rotate.last_time(),
            t.translate.last_time(),
            t.scale.last_time(),
        ]
    });
    let slot_times = slot_timelines.values().map(|t| t.last_time());
    bone_times
        .chain(slot_times)
        .chain([events_timeline.last_time()])
        .flatten()
        .fold(0.0, f32::max)
}

impl SpineSkeletonBuilder<'_> {
    /// Creates the bone, slot and event timelines of one animation.
    pub(super) fn load_animation(
        &mut self,
        spine_animation: &spine_json::Animation,
    ) -> Result<Animation> {
        // Bone transformations

        let bone_timelines = spine_animation
            .bones
            .iter()
            .map(|(bone_name, bone_animation)| {
                let timelines = BoneTimelines {
                    rotate: CurveTimeline::new(bone_animation.rotate.iter().map(KeyDef::from)),
                    translate: CurveTimeline::new(
                        bone_animation.translate.iter().map(KeyDef::from),
                    ),
                    scale: CurveTimeline::new(bone_animation.scale.iter().map(KeyDef::from)),
                };
                Ok((self.bone(bone_name)?, timelines))
            })
            .collect::<Result<HashMap<_, _>>>()?;

        // Slot/attachment associations

        let mut slot_timelines = HashMap::new();
        for (slot_name, SlotTimelines { attachment, .. }) in &spine_animation.slots {
            let slot_index = self.slot(slot_name)?.index;
            let attachment_timeline = Timeline::new(attachment.iter().map(|t| {
                (
                    t.time,
                    match &t.name {
                        // None => slots[slot_name].default_attachment,
                        None => AttachmentIndex::INVALID,
                        Some(attachment_name) => self.registry.register_if_in_skin(
                            &self.skin,
                            slot_index,
                            slot_name,
                            attachment_name,
                        ),
                    },
                )
            }));
            slot_timelines.insert(slot_index, attachment_timeline);
        }

        // Events

        let events_timeline = Timeline::new(
            spine_animation
                .events
                .iter()
                .map(|ev| (ev.time, ev.name.clone())),
        );

        Ok(Animation {
            duration: animation_duration(&bone_timelines, &slot_timelines, &events_timeline),
            bone_timelines,
            slot_timelines,
            events_timeline,
        })
    }
}

/// Convert the `curve` field of a JSON key into one [`CurveDef`] per value component (one for
/// rotate, two for translate/scale: x then y).
///
/// A missing curve is linear. A bezier array with the wrong number of values (it must hold 4 per
/// component) is malformed; we warn and fall back to linear rather than failing the whole load.
fn curve_defs<const N: usize>(curve: Option<&Curve>) -> [CurveDef; N] {
    match curve {
        None => [CurveDef::Linear; N],
        Some(Curve::Named(name)) => {
            if name != "stepped" && name != "linear" {
                warn!("Unknown Spine curve {name:?}, treating it as linear");
            }
            [if name == "stepped" {
                CurveDef::Stepped
            } else {
                CurveDef::Linear
            }; N]
        }
        Some(Curve::Bezier(points)) if points.len() == 4 * N => {
            std::array::from_fn(|c| CurveDef::Bezier(points[4 * c..4 * c + 4].try_into().unwrap()))
        }
        Some(Curve::Bezier(points)) => {
            warn!(
                "Spine bezier curve has {} control values, expected {}; treating it as linear",
                points.len(),
                4 * N
            );
            [CurveDef::Linear; N]
        }
    }
}

impl From<&RotateTimelineEntry> for KeyDef<1> {
    fn from(t: &RotateTimelineEntry) -> Self {
        KeyDef {
            time: t.time,
            value: [t.value],
            curve: curve_defs(t.curve.as_ref()),
        }
    }
}

impl From<&TranslateTimelineEntry> for KeyDef<2> {
    fn from(t: &TranslateTimelineEntry) -> Self {
        KeyDef {
            time: t.time,
            value: [t.x, t.y],
            curve: curve_defs(t.curve.as_ref()),
        }
    }
}

impl From<&ScaleTimelineEntry> for KeyDef<2> {
    fn from(t: &ScaleTimelineEntry) -> Self {
        KeyDef {
            time: t.time,
            value: [t.x, t.y],
            curve: curve_defs(t.curve.as_ref()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curve_defs_maps_json_curves_per_component() {
        assert_eq!(curve_defs::<1>(None), [CurveDef::Linear]);
        assert_eq!(
            curve_defs::<2>(Some(&Curve::Named("stepped".into()))),
            [CurveDef::Stepped; 2]
        );
        assert_eq!(
            curve_defs::<1>(Some(&Curve::Bezier(vec![0.1, 0.2, 0.3, 0.4]))),
            [CurveDef::Bezier([0.1, 0.2, 0.3, 0.4])]
        );
        assert_eq!(
            curve_defs::<2>(Some(&Curve::Bezier(vec![
                0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8
            ]))),
            [
                CurveDef::Bezier([0.1, 0.2, 0.3, 0.4]),
                CurveDef::Bezier([0.5, 0.6, 0.7, 0.8])
            ]
        );
        // Wrong control point count falls back to linear.
        assert_eq!(
            curve_defs::<2>(Some(&Curve::Bezier(vec![0.1, 0.2, 0.3, 0.4]))),
            [CurveDef::Linear; 2]
        );
    }
}
