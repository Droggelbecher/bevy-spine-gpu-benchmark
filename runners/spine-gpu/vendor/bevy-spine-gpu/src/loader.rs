use bevy::{ecs::system::EntityCommands, math::Affine2, prelude::*};

use crate::{DEFAULT_ANIMATION_TRANSITION_DURATION, DEFAULT_RENDER_BUCKET};

mod asset;
mod build;
mod spine_json;
mod system;
#[cfg(test)]
mod tests;

pub(crate) use asset::{SpineAtlasLoader, SpineJsonLoader};
pub(crate) use system::{LoadSpineCache, PendingSpine, finish_pending_spine, load_spine};

/// Builds the default skin of a skeleton from the contents of its files, without asset server
/// (for tests outside of this module).
#[cfg(test)]
pub(crate) fn build_skeleton_for_tests(
    path: &str,
    json: &str,
    atlas: crate::Atlas,
) -> crate::SpineSkeleton {
    build::parse_spine_file(path, json)
        .unwrap()
        .build(None, atlas)
        .unwrap()
        .into_asset()
}

// --------------------------------------------------------------------------------
// Public API
// --------------------------------------------------------------------------------

/// Assigns the slots of all loaded skeletons whose name starts with `prefix` to a draw phase,
/// see [`SpineGPUPlugin::phases`](crate::SpineGPUPlugin::phases). Of several matching rules the
/// first one applies; slots without a matching rule are in phase 0.
#[derive(Clone, Debug)]
pub struct SlotRule {
    pub prefix: String,
    /// Index into [`SpineGPUPlugin::phases`](crate::SpineGPUPlugin::phases)
    pub phase: usize,
}

/// A [`LoadSpine`] request and its settings, as one value.
#[derive(Default, Debug)]
pub struct LoadSpineBundle {
    pub loadspine: Option<LoadSpine>,
    pub settings: LoadSpineSettings,
}

/// Existence of this component requests loading a spine animation.
/// The component is removed right away. Once the skeleton and its texture have been loaded,
/// other spine-related components ([`SkeletonInstance`](crate::SkeletonInstance), [`Loaded`], ...)
/// are attached to the entity. Loading happens through the asset server, so the skeleton
/// type is a [`SpineSkeleton`](crate::SpineSkeleton) asset. Requests that fail are logged.
/// Eg `LoadSpine::SpineDirectory("animations/example01/Spine".into(), None)`
/// or `LoadSpine::SpineDirectory("animations/example01_skins/Spine".into(), Some("alt".into()))`.
///
/// `SpineDirectory(path, skin)` loads `<path>.json` with the `<path>.atlas` next to it. Selecting
/// a skin by name is possible for files with more than one skin (otherwise there is only the
/// default one, `None`).
#[derive(Component, Clone, PartialEq, Eq, Hash, Debug)]
pub enum LoadSpine {
    SpineDirectory(String, Option<String>),
    /// Create a spine "animation" from a simple static image and size. Ref point will be bottom center.
    StaticImage(Handle<Image>, UVec2),
    StaticImagePath(String, UVec2),
    /// Path to .atlas file
    Atlas(String),
}

/// Records a successfully loaded spine animation on an entity
#[derive(Component, Debug)]
pub struct Loaded {
    pub loadspine: LoadSpine,
    pub settings: Option<LoadSpineSettings>,
}

#[derive(Component, Clone, Debug)]
pub struct LoadSpineSettings {
    pub animation: String,
    pub color: Vec4,
    pub time_scale: f32,
    pub transition_duration: f32,
    /// Lower buckets are drawn first; higher buckets are drawn later and therefore appear on top.
    /// Ignored for `sorted` types, which are always drawn after all buckets.
    /// This is a property of the skeleton *type*, applied when the `LoadSpine` is first loaded;
    /// later requests with a different value are ignored (with a warning).
    pub render_bucket: i32,
    /// Draw this skeleton *type* in the sorted group: after all other types, with its instances
    /// ordered across all sorted types by `SkeletonInstance::sort_key` (higher is in front)
    /// instead of one draw per type. This is a property of the type, not the instance: it is
    /// applied when the type is first loaded; later requests with a different value are ignored
    /// (with a warning).
    pub sorted: bool,
    /// Apply this 2d transform before global transform and root bone transform.
    /// This is useful for doing things like scaling or offsetting root bone
    pub base_transform: Affine2,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AttachmentIndex(pub(crate) u32);

// --------------------------------------------------------------------------------
// Impls
// --------------------------------------------------------------------------------

impl LoadSpineBundle {
    /// Insert the request and its settings into the entity, if there is a request.
    pub fn insert_into(&self, ec: &mut EntityCommands) {
        if let Some(loadspine) = &self.loadspine {
            ec.try_insert(loadspine.clone());
            ec.try_insert(self.settings.clone());
        }
    }

    pub fn with_color(mut self, color: Vec4) -> Self {
        self.settings.color = color;
        self
    }

    pub fn with_time_scale(mut self, timescale: f32) -> Self {
        self.settings.time_scale = timescale;
        self
    }

    pub fn with_transition_duration(mut self, duration: f32) -> Self {
        self.settings.transition_duration = duration.max(0.0);
        self
    }

    /// Draw this skeleton type in the sorted group (see `LoadSpineSettings::sorted`).
    /// Applies to the whole type, when it is first loaded; later conflicting requests are ignored.
    pub fn with_sorted(mut self, sorted: bool) -> Self {
        self.settings.sorted = sorted;
        self
    }

    /// Draw this skeleton type in the given render bucket (see `LoadSpineSettings::render_bucket`).
    /// Applies to the whole type, when it is first loaded; later conflicting requests are ignored.
    pub fn with_render_bucket(mut self, bucket: i32) -> Self {
        self.settings.render_bucket = bucket;
        self
    }

    pub fn with_translation(mut self, translation: Vec2) -> Self {
        self.settings.base_transform.translation += translation;
        self
    }

    /// Compose `transform` into the base transform in the skeleton's local frame
    /// (right-multiply): it is applied to the art first, before the existing base transform.
    pub fn with_local_transform(mut self, transform: Affine2) -> Self {
        self.settings.base_transform *= transform;
        self
    }

    /// Compose `transform` into the base transform in the entity's frame
    /// (left-multiply): it is applied to the art last, after the existing base transform.
    pub fn with_outer_transform(mut self, transform: Affine2) -> Self {
        self.settings.base_transform = transform * self.settings.base_transform;
        self
    }

    pub fn into_option(self) -> Option<(LoadSpine, LoadSpineSettings)> {
        match self.loadspine {
            Some(loadspine) => Some((loadspine, self.settings)),
            _ => None,
        }
    }
}

impl LoadSpine {
    pub fn spine_directory(path: impl Into<String>) -> Self {
        Self::SpineDirectory(path.into(), None)
    }

    pub fn spine_directory_with_skin(path: impl Into<String>, skin: impl Into<String>) -> Self {
        Self::SpineDirectory(path.into(), Some(skin.into()))
    }
}

impl Default for LoadSpineSettings {
    fn default() -> Self {
        Self {
            animation: Default::default(),
            color: Vec4::ONE,
            time_scale: 1.0,
            transition_duration: DEFAULT_ANIMATION_TRANSITION_DURATION,
            render_bucket: DEFAULT_RENDER_BUCKET,
            sorted: false,
            base_transform: Affine2::IDENTITY,
        }
    }
}

impl AttachmentIndex {
    pub(crate) const INVALID: AttachmentIndex = AttachmentIndex(u32::MAX);
}
