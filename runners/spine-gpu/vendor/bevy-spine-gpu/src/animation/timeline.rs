use bevy::math::Vec2;
use smallvec::SmallVec;

use super::CurveTimeline;

/// Plain keyframe list for slot attachment and event timelines. These are inherently stepped, so
/// they need no interpolation; numeric bone timelines use [`CurveTimeline`] instead.
#[derive(Clone, Default, Debug)]
pub(crate) struct Timeline<T> {
    pub(crate) entries: Vec<(f32, T)>,
}

#[derive(Clone, Default, Debug)]
pub(crate) struct BoneTimelines {
    /// Rotation in degrees (added to the bone's setup rotation).
    pub(crate) rotate: CurveTimeline<1>,
    pub(crate) translate: CurveTimeline<2>,
    pub(crate) scale: CurveTimeline<2>,
}

impl<T> Timeline<T> {
    pub fn new(entries: impl Iterator<Item = (f32, T)>) -> Self {
        Self {
            entries: entries.collect(),
        }
    }

    pub fn empty() -> Self {
        Self { entries: vec![] }
    }

    pub(crate) fn last_time(&self) -> Option<f32> {
        self.entries.last().map(|(time, _)| *time)
    }

    /// Get latest value at or before the given time index.
    /// If `time` is before first timeline entry, return None.
    pub(super) fn get_latest_value(&self, time: f32) -> Option<T>
    where
        T: Copy + std::fmt::Debug,
    {
        let right = self.entries.partition_point(|&(t, _)| t <= time);
        if right == 0 {
            return None;
        }
        let left = right - 1;
        Some(self.entries[left].1)
    }

    /// Entries with time in `[time0, time1)`. If `time1 < time0`, the animation wrapped around
    /// and the range is `[time0, end] ∪ [0, time1)`. `time0 == time1` means time did not
    /// advance, so the range is empty.
    pub(super) fn iter_range(&self, time0: f32, time1: f32) -> impl Iterator<Item = (f32, T)>
    where
        T: Clone,
    {
        let events: SmallVec<[(f32, T); 4]> = if time0 <= time1 {
            self.entries
                .iter()
                .filter(|(t, _)| *t >= time0 && *t < time1)
                .cloned()
                .collect::<SmallVec<_>>()
        } else {
            self.entries
                .iter()
                .filter(|(t, _)| *t >= time0 || *t < time1)
                .cloned()
                .collect()
        };
        events.into_iter()
    }
}

impl BoneTimelines {
    pub(super) fn get_rotation(&self, t: f32) -> f32 {
        self.rotate.at(t).map_or(0.0, |[degrees]| degrees)
    }

    pub(super) fn get_translation(&self, t: f32) -> Vec2 {
        self.translate.at(t).map_or(Vec2::ZERO, Vec2::from)
    }

    pub(super) fn get_scale(&self, t: f32) -> Vec2 {
        self.scale.at(t).map_or(Vec2::ONE, Vec2::from)
    }
}
