use std::collections::HashMap;

use bevy::prelude::*;

use super::{AnimationTransition, BoneOverride, DEFAULT_ANIMATION_TRANSITION_DURATION, SlotIndex};
use crate::{AttachmentIndex, BoneIndex};

/// Settings for the skeleton animation of an entity: label, speed, overrides, etc..
///
/// This is the component application code writes. bevy_spine_gpu only ever reads it (see
/// [`AnimationPlayback`] for the state bevy_spine_gpu advances every frame), so
/// `Changed<AnimationControl>` means that application code changed something.
#[derive(Component, Clone, Debug)]
pub struct AnimationControl {
    pub(crate) label: String,
    /// Cross-fade duration to use for the most recent label change.
    pub(crate) label_transition: f32,
    pub(crate) transition_duration: f32,
    pub(crate) time_scale: f32,
    pub(crate) repeat: bool,
    // slot index -> attachment index
    pub(crate) attachment_overrides: HashMap<SlotIndex, AttachmentIndex>,
    // bone index -> override applied on top of setup pose + animation
    pub(crate) bone_overrides: HashMap<BoneIndex, BoneOverride>,
}

impl Default for AnimationControl {
    fn default() -> Self {
        Self {
            label: "".into(),
            label_transition: 0.0,
            transition_duration: DEFAULT_ANIMATION_TRANSITION_DURATION,
            time_scale: 1.0,
            repeat: true,
            attachment_overrides: HashMap::new(),
            bone_overrides: HashMap::new(),
        }
    }
}

/// Playback state of the skeleton animation of an entity: current time, running transition, etc..
///
/// This is written by bevy_spine_gpu every frame the entity is animated, so `Changed<AnimationPlayback>`
/// is true every frame for animated entities. The only write intended for application code is seeking via
/// [`Self::set_current_time_seconds`]. Everything else is configured through [`AnimationControl`].
#[derive(Component, Clone, Debug, Default)]
pub struct AnimationPlayback {
    pub(crate) time: f32,
    /// The label currently playing, i.e. last synced from [`AnimationControl`].
    pub(crate) label: String,
    pub(crate) transition: Option<AnimationTransition>,
}

impl AnimationPlayback {
    /// Set the current animation time directly in animation seconds (seek).
    ///
    /// This is the one write to this component intended for application code, e.g. to restart or scrub
    /// an animation. Note: This *will* mark the component modified (which it is every frame for
    /// animated entities anyway).
    pub fn set_current_time_seconds(&mut self, t: f32) {
        self.time = t.max(0.0);
    }

    pub fn current_time_seconds(&self) -> f32 {
        self.time
    }

    /// The label currently playing. Lags behind [`AnimationControl::label`] until the next
    /// `advance_animations` run.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Whether a cross-fade to the current animation is running.
    pub fn is_transitioning(&self) -> bool {
        self.transition.is_some()
    }
}

impl AnimationControl {
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Override the attachment shown for a slot.
    ///
    /// Overrides are applied after the animation's slot timeline. Clear the override to return to
    /// the animated/default attachment.
    pub fn set_slot_attachment(&mut self, slot: SlotIndex, attachment: AttachmentIndex) {
        self.attachment_overrides.insert(slot, attachment);
    }

    /// Hide a slot until its override is cleared.
    pub fn hide_slot(&mut self, slot: SlotIndex) {
        self.attachment_overrides
            .insert(slot, AttachmentIndex::INVALID);
    }

    /// Return the active override for a slot, if one has been set.
    pub fn slot_attachment_override(&self, slot: SlotIndex) -> Option<AttachmentIndex> {
        self.attachment_overrides.get(&slot).copied()
    }

    /// Clear the attachment override for a slot.
    pub fn clear_slot_attachment_override(&mut self, slot: SlotIndex) {
        self.attachment_overrides.remove(&slot);
    }

    /// Clear all attachment overrides.
    pub fn clear_slot_attachment_overrides(&mut self) {
        self.attachment_overrides.clear();
    }

    /// Override a bone's local translation offset.
    ///
    /// The offset is added to the bone's setup pose and animation, so child bones follow and
    /// animation transitions blend it like any other pose change. Clear the override to return to
    /// the animated pose.
    ///
    /// Note: This *will* mark the component modified, even if the value is unchanged. Callers
    /// holding a `Mut<AnimationControl>` should compare with [`Self::bone_override`] first.
    pub fn set_bone_override(&mut self, bone: BoneIndex, bone_override: BoneOverride) {
        self.bone_overrides.insert(bone, bone_override);
    }

    /// Return the active override for a bone, if one has been set.
    pub fn bone_override(&self, bone: BoneIndex) -> Option<BoneOverride> {
        self.bone_overrides.get(&bone).copied()
    }

    /// Clear the override for a bone.
    ///
    /// Note: This *will* mark the component modified, even if there was no override. Check
    /// [`Self::bone_override`] first to avoid that.
    pub fn clear_bone_override(&mut self, bone: BoneIndex) {
        self.bone_overrides.remove(&bone);
    }

    /// Change current animation to given label.
    ///
    /// This starts a pose transition using [`Self::transition_duration`]. It intentionally does
    /// not reset the animation time; callers that want the new animation to restart should call
    /// [`AnimationPlayback::set_current_time_seconds`] as well.
    ///
    /// Note: This *will* mark the component modified. If you want a way to ensure state of your
    /// animation that only updates when necessary, check out `SpineGPUQuery`.
    pub fn set_label(&mut self, a: impl Into<String>) {
        self.set_label_with_transition(a, self.transition_duration);
    }

    /// Change current animation to given label with an explicit transition duration.
    ///
    /// A duration of `0.0` switches immediately.
    pub fn set_label_with_transition(&mut self, a: impl Into<String>, duration: f32) {
        let label = a.into();
        if self.label == label {
            return;
        }

        self.label = label;
        self.label_transition = duration.max(0.0);
    }

    // Larger means faster
    pub fn time_scale(&self) -> f32 {
        self.time_scale
    }

    /// Larger means faster
    /// Note: This *will* mark the component modified. If you want a way to ensure state of your
    /// animation that only updates when necessary, check out `SpineGPUQuery`.
    pub fn set_time_scale(&mut self, s: f32) {
        self.time_scale = s;
    }

    /// Duration, in seconds, used by [`Self::set_label`] when cross-fading to a new animation.
    pub fn transition_duration(&self) -> f32 {
        self.transition_duration
    }

    /// Set the default cross-fade duration for future [`Self::set_label`] calls.
    ///
    /// A duration of `0.0` disables transitions. This does not cancel a transition that is
    /// already running; that state lives in [`AnimationPlayback`].
    pub fn set_transition_duration(&mut self, duration: f32) {
        self.transition_duration = duration.max(0.0);
    }

    pub fn repeat(&self) -> bool {
        self.repeat
    }

    pub fn set_repeat(&mut self, repeat: bool) {
        self.repeat = repeat;
    }
}
