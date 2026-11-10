//! Parser for Spine JSON files.
//! The format is specified in https://de.esotericsoftware.com/spine-json-format
//!
use std::collections::HashMap;

use serde::{Deserialize, Serialize};

mod attachment;
mod skin;

pub(crate) use attachment::Attachment;
pub(crate) use skin::EffectiveSkin;

pub(super) const DEFAULT_SKIN_NAME: &str = "default";

/// A Spine color, `"RRGGBBAA"` (or `"RRGGBB"`) in the JSON. Defaults to opaque white.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SpineColor(pub(crate) [u8; 4]);

impl SpineColor {
    pub(crate) const WHITE: SpineColor = SpineColor([255; 4]);

    pub(crate) fn parse(s: &str) -> Option<Self> {
        if !s.is_ascii() || !(s.len() == 6 || s.len() == 8) {
            return None;
        }
        let mut c = [255u8; 4];
        for (i, v) in c.iter_mut().enumerate().take(s.len() / 2) {
            *v = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
        }
        Some(SpineColor(c))
    }

    /// Straight-alpha components in 0..1.
    pub(crate) fn to_f32(self) -> [f32; 4] {
        self.0.map(|c| c as f32 / 255.0)
    }
}

impl Default for SpineColor {
    fn default() -> Self {
        Self::WHITE
    }
}

impl Serialize for SpineColor {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let [r, g, b, a] = self.0;
        serializer.serialize_str(&format!("{r:02x}{g:02x}{b:02x}{a:02x}"))
    }
}

impl<'de> Deserialize<'de> for SpineColor {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        SpineColor::parse(&s).ok_or_else(|| {
            serde::de::Error::custom(format!("invalid color {s:?}, expected RRGGBBAA"))
        })
    }
}

/// Spine 4.2 JSON Format
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct SpineJson {
    pub(super) skeleton: Skeleton,
    pub(super) bones: Vec<Bone>,
    pub(super) slots: Vec<Slot>,
    pub(super) skins: Vec<Skin>,
    pub(super) animations: HashMap<String, Animation>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct Skeleton {
    hash: String,
    spine: String,
    pub(super) width: f32,
    pub(super) height: f32,
    #[serde(default = "zerof32")]
    pub(super) x: f32,
    #[serde(default = "zerof32")]
    pub(super) y: f32,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct Bone {
    pub(super) name: String,
    pub(super) parent: Option<String>,
    pub(super) rotation: Option<f32>,
    pub(super) x: Option<f32>,
    pub(super) y: Option<f32>,
    pub(super) length: Option<f32>,
    #[serde(rename = "scaleX", default = "default_scale")]
    pub(super) scale_x: f32,
    #[serde(rename = "scaleY", default = "default_scale")]
    pub(super) scale_y: f32,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct Skin {
    pub(super) name: String,
    // slot_name -> attachment_name -> attachment
    pub(super) attachments: HashMap<String, HashMap<String, Attachment>>,
}

// Helper for serde to force a string of "mesh"
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "lowercase")]
pub(super) enum MeshType {
    Mesh,
}

// Helper for serde to force a string of "linkedmesh"
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "lowercase")]
pub(super) enum LinkedMeshType {
    LinkedMesh,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct Slot {
    pub(super) name: String,
    pub(super) bone: String,
    pub(super) attachment: Option<String>,
    /// Setup color of the slot. (The two-color tint `dark` is not supported and ignored.)
    #[serde(default)]
    pub(super) color: SpineColor,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct Animation {
    // bone_name -> timeline_type -> timeline
    #[serde(default)]
    pub(super) bones: HashMap<String, BoneTimelines>,
    // slot_name -> timeline_type -> timeline
    #[serde(default)]
    pub(super) slots: HashMap<String, SlotTimelines>,

    #[serde(default)]
    pub(super) events: Vec<EventTimelineEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct EventTimelineEntry {
    #[serde(default)]
    pub(super) time: f32,
    pub(super) name: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct SlotTimelines {
    #[serde(default)]
    pub(super) attachment: Vec<AttachmentTimelineEntry>,
    // Color timelines are not supported, only detected (to warn), contents are not parsed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rgba: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rgb: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    alpha: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rgba2: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rgb2: Option<serde_json::Value>,
}

impl SlotTimelines {
    /// Whether the slot animates its color, which is not supported.
    pub(super) fn has_color_timelines(&self) -> bool {
        [&self.rgba, &self.rgb, &self.alpha, &self.rgba2, &self.rgb2]
            .iter()
            .any(|t| t.is_some())
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct BoneTimelines {
    #[serde(default)]
    pub(super) rotate: Vec<RotateTimelineEntry>,
    #[serde(default)]
    pub(super) translate: Vec<TranslateTimelineEntry>,
    #[serde(default)]
    pub(super) scale: Vec<ScaleTimelineEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct AttachmentTimelineEntry {
    #[serde(default)]
    pub(super) time: f32,
    pub(super) name: Option<String>,
}

/// The `curve` field of a bone timeline key, describing the segment to the next key.
/// Absent means linear. See `animation/curve.rs` for the semantics.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(untagged)]
pub(super) enum Curve {
    /// `"stepped"` (Spine 4.x writes no other names; `"linear"` is accepted too).
    Named(String),
    /// Absolute bezier control points, 4 numbers per value component:
    /// `cx1, cy1, cx2, cy2` (x component first for translate/scale).
    Bezier(Vec<f32>),
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct RotateTimelineEntry {
    #[serde(default)]
    pub(super) time: f32,
    #[serde(default)]
    pub(super) value: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) curve: Option<Curve>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct TranslateTimelineEntry {
    #[serde(default)]
    pub(super) time: f32,
    #[serde(default)]
    pub(super) x: f32,
    #[serde(default)]
    pub(super) y: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) curve: Option<Curve>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct ScaleTimelineEntry {
    #[serde(default)]
    pub(super) time: f32,
    #[serde(default = "default_scale")]
    pub(super) x: f32,
    #[serde(default = "default_scale")]
    pub(super) y: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) curve: Option<Curve>,
}

pub(crate) fn default_scale() -> f32 {
    1.0
}
pub(crate) fn zerof32() -> f32 {
    0.0
}

impl SpineJson {
    /// Whether any animation animates a slot color (not supported, the setup color is used).
    pub(super) fn has_color_timelines(&self) -> bool {
        self.animations
            .values()
            .any(|a| a.slots.values().any(SlotTimelines::has_color_timelines))
    }
}

pub(super) fn parse(data: &str) -> serde_json::Result<SpineJson> {
    serde_json::from_str(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_bones(timelines: &str) -> BoneTimelines {
        serde_json::from_str(timelines).unwrap()
    }

    #[test]
    fn absent_curve_is_none() {
        let b = parse_bones(r#"{"rotate": [{"time": 0.5, "value": 3}], "translate": [{"x": 1}]}"#);
        assert_eq!(b.rotate[0].curve, None);
        assert_eq!(b.translate[0].curve, None);
    }

    #[test]
    fn parses_stepped_curve() {
        let b = parse_bones(
            r#"{"rotate": [{"time": 0.4666667, "value": 63.12949, "curve": "stepped"}]}"#,
        );
        assert_eq!(b.rotate[0].curve, Some(Curve::Named("stepped".into())));
    }

    #[test]
    fn parses_four_number_bezier_with_defaulted_time_and_value() {
        let b = parse_bones(r#"{"rotate": [{"curve": [0.224, 0, 0.447, -7.64]}]}"#);
        assert_eq!(b.rotate[0].time, 0.0);
        assert_eq!(b.rotate[0].value, 0.0);
        assert_eq!(
            b.rotate[0].curve,
            Some(Curve::Bezier(vec![0.224, 0.0, 0.447, -7.64]))
        );
    }

    #[test]
    fn parses_eight_number_bezier() {
        let b = parse_bones(
            r#"{"translate": [{"curve": [0.211, 0, 0.456, 0, 0.211, 0, 0.456, -0.67]}],
                "scale": [{"curve": [0.1, 1, 0.2, 1, 0.3, 1, 0.4, 2]}]}"#,
        );
        assert_eq!(
            b.translate[0].curve,
            Some(Curve::Bezier(vec![
                0.211, 0.0, 0.456, 0.0, 0.211, 0.0, 0.456, -0.67
            ]))
        );
        assert!(matches!(&b.scale[0].curve, Some(Curve::Bezier(v)) if v.len() == 8));
    }
}
