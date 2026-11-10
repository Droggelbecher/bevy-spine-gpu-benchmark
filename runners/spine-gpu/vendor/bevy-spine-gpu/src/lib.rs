//! A GPU Spine2D renderer for Bevy 0.14.
//!
//! The vertices of a skeleton type live in a static storage buffer; the vertex shader pulls them,
//! skins them with the bone transforms of the instance and tints them. Every frame the headers
//! and bones of all instances are packed into two storage buffers, and each skeleton type is
//! drawn with a single instanced draw call. Skeleton types of the *sorted* group are drawn in
//! y-sorted order across types.
//!
//! ```no_run
//! use bevy::prelude::*;
//! use bevy_spine_gpu::{AnimationControl, LoadSpine, SpineGPUPlugin};
//!
//! fn main() {
//!     App::new()
//!         .add_plugins((DefaultPlugins, SpineGPUPlugin::default()))
//!         .add_systems(Startup, setup)
//!         .add_systems(Update, play_animation)
//!         .run();
//! }
//!
//! fn setup(mut commands: Commands) {
//!     commands.spawn((
//!         LoadSpine::spine_directory("animations/ella/Ella"),
//!         SpatialBundle::default(),
//!     ));
//!     commands.spawn(Camera2dBundle::default());
//! }
//!
//! // `AnimationControl` is added once the skeleton is loaded
//! fn play_animation(mut query: Query<&mut AnimationControl, Added<AnimationControl>>) {
//!     for mut animation in &mut query {
//!         animation.set_label("walk_l");
//!     }
//! }
//! ```

mod animation;
mod atlas;
mod attributes;
mod debugging;
mod loader;
mod plugin;
mod query;
mod render;
mod skeleton;

// Plugin and configuration
pub use plugin::{SpineGPUPlugin, SpineGPUSystem};

// Loading skeleton types
pub use loader::{LoadSpine, LoadSpineBundle, LoadSpineSettings, Loaded, SlotRule};

// Skeleton instances, reading type information and animation control
pub use animation::{
    AlwaysAnimate, AnimationControl, AnimationPlayback, AttachmentOverrideError, BoneOverride,
    SlotIndex,
};
pub use loader::AttachmentIndex;
pub use skeleton::{BoneIndex, SkeletonInfo, SkeletonInstance, SpineSkeleton};

// Draw phases
pub use render::SpineDrawPhases;

// Queries
pub use query::SpineGPUQuery;

// Events
pub use animation::SpineEvent;

// Debugging
pub use debugging::SpineGPUDebugging;

pub(crate) use animation::*;
pub(crate) use atlas::*;
pub(crate) use attributes::*;
pub(crate) use debugging::*;
pub(crate) use loader::*;
pub(crate) use plugin::*;
pub(crate) use render::*;
pub(crate) use skeleton::*;
