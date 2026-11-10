use bevy::{prelude::*, transform::TransformSystem};

use crate::{
    LoadSpineCache, RenderPlugin, SlotRule, SpineAtlasLoader, SpineEvent, SpineGPUDebugging,
    SpineJsonLoader, SpineSkeleton, advance_animations, compute_setup_pose, debug_aabb,
    debug_draw_bones, finish_pending_spine, load_spine, update_aabb, update_sort_keys,
};

pub struct SpineGPUPlugin {
    /// The z (among Bevy's transparent 2d items) of each draw phase. Every phase draws its slots
    /// of all skeletons, so a phase with a higher z is in front of all skeletons of lower ones.
    /// Slots are in phase 0 unless a [`SlotRule`] says otherwise. Phases can be hidden via
    /// [`SpineDrawPhases`](crate::SpineDrawPhases).
    pub phases: Vec<f32>,
    /// Draw phase of slots by slot name prefix, for all loaded `.json` skeletons
    pub slot_rules: Vec<SlotRule>,
    /// Pad AABB around setup pose by this many times the largest side length
    pub aabb_padding: f32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, SystemSet)]
pub enum SpineGPUSystem {
    UpdateAnimation,
}

#[derive(Resource)]
pub(crate) struct SpineGPUConfig {
    pub(crate) aabb_padding: f32,
}

impl Plugin for SpineGPUPlugin {
    fn build(&self, app: &mut App) {
        assert!(
            !self.phases.is_empty(),
            "SpineGPUPlugin needs at least one phase"
        );
        for rule in &self.slot_rules {
            assert!(
                rule.phase < self.phases.len(),
                "slot rule {rule:?} refers to a phase that doesn't exist"
            );
        }

        app.init_asset::<SpineSkeleton>()
            .register_asset_loader(SpineJsonLoader {
                slot_rules: self.slot_rules.clone().into(),
            })
            .init_asset_loader::<SpineAtlasLoader>()
            .add_plugins(RenderPlugin {
                phases: self.phases.clone(),
            })
            .init_resource::<LoadSpineCache>()
            .init_resource::<SpineGPUDebugging>()
            .insert_resource(SpineGPUConfig {
                aabb_padding: self.aabb_padding,
            })
            .add_event::<SpineEvent>()
            .configure_sets(
                PostUpdate,
                SpineGPUSystem::UpdateAnimation.after(TransformSystem::TransformPropagate),
            )
            .add_systems(Update, (load_spine, finish_pending_spine).chain())
            .add_systems(
                PostUpdate,
                (
                    update_sort_keys,
                    compute_setup_pose,
                    update_aabb,
                    advance_animations,
                )
                    .chain()
                    .in_set(SpineGPUSystem::UpdateAnimation),
            )
            .add_systems(
                PostUpdate,
                debug_draw_bones.after(SpineGPUSystem::UpdateAnimation),
            )
            .add_systems(
                PostUpdate,
                debug_aabb.after(SpineGPUSystem::UpdateAnimation),
            );
    }
}

impl Default for SpineGPUPlugin {
    fn default() -> Self {
        Self {
            phases: vec![0.0],
            slot_rules: Vec::new(),
            aabb_padding: 1.0,
        }
    }
}
