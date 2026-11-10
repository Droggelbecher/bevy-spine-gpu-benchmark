use bevy::{
    core_pipeline::core_2d::Transparent2d,
    prelude::*,
    render::{
        Extract, Render, RenderApp, RenderSet, render_asset::RenderAssetPlugin,
        render_phase::AddRenderCommand, render_resource::SpecializedRenderPipelines,
        texture::GpuImage,
    },
};

mod draw;
mod draw_order;
mod gpu_skeleton;
mod instance_infos;
mod pipeline;
mod queue;
mod vertex_info;

pub(crate) use gpu_skeleton::GpuSpineSkeleton;
pub(crate) use instance_infos::*;
use queue::*;
pub(crate) use vertex_info::*;

use crate::{
    SkeletonDrawGroups, finish_pending_spine,
    render::{
        draw::DrawSpineGpu,
        draw_order::*,
        pipeline::{SPINE_GPU_SHADER_HANDLE, SpineGPUPipeline},
    },
};

pub(crate) struct RenderPlugin {
    pub(crate) phases: Vec<f32>,
}

/// The draw phases of the skeletons (see [`SpineGPUPlugin::phases`](crate::SpineGPUPlugin)) and
/// whether each of them is drawn. All phases start enabled.
#[derive(Resource, Clone, Debug)]
pub struct SpineDrawPhases {
    z: Vec<f32>,
    enabled: Vec<bool>,
}

impl SpineDrawPhases {
    pub(crate) fn new(z: Vec<f32>) -> Self {
        Self {
            enabled: vec![true; z.len()],
            z,
        }
    }

    /// Whether the slots of `phase` are drawn; false for phases that don't exist.
    pub fn is_enabled(&self, phase: usize) -> bool {
        self.enabled.get(phase).copied().unwrap_or(false)
    }

    /// Draw or hide the slots of `phase` (of all skeletons).
    ///
    /// # Panics
    /// If the phase doesn't exist.
    pub fn set_enabled(&mut self, phase: usize, enabled: bool) {
        self.enabled[phase] = enabled;
    }

    /// Index and z of the enabled phases
    fn enabled_phases(&self) -> impl Iterator<Item = (usize, f32)> + '_ {
        self.z
            .iter()
            .enumerate()
            .filter(|&(i, _)| self.enabled[i])
            .map(|(i, &z)| (i, z))
    }
}

/// Render world entity of an enabled draw phase, (re)spawned every frame. The phase item of
/// the phase points to it, so the draw command knows which indices to draw.
#[derive(Component)]
pub(crate) struct ExtractedDrawPhase {
    pub(crate) index: usize,
    pub(crate) z: f32,
}

impl Plugin for RenderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<InstanceInfos>()
            .init_resource::<SkeletonDrawGroups>()
            .insert_resource(SpineDrawPhases::new(self.phases.clone()))
            .add_plugins(RenderAssetPlugin::<GpuSpineSkeleton, GpuImage>::default())
            .add_systems(
                Update,
                (add_skeleton_instance, remove_skeleton_instance).after(finish_pending_spine),
            );

        let mut shaders = app.world_mut().resource_mut::<Assets<Shader>>();
        shaders.insert(
            &SPINE_GPU_SHADER_HANDLE,
            Shader::from_wgsl(include_str!("render/spine_gpu.wgsl"), "spine_gpu.wgsl"),
        );

        app.get_sub_app_mut(RenderApp)
            .unwrap()
            .add_render_command::<Transparent2d, DrawSpineGpu>()
            .init_resource::<SpecializedRenderPipelines<SpineGPUPipeline>>()
            .init_resource::<ExtractedInstances>()
            .init_resource::<InstanceLayout>()
            .init_resource::<InstanceInfosBuffers>()
            .init_resource::<SortedDraw>()
            .init_resource::<SkeletonDrawGroups>()
            .add_systems(
                ExtractSchedule,
                (
                    extract_instance_infos,
                    extract_draw_groups,
                    extract_draw_phases,
                ),
            )
            .add_systems(
                Render,
                // The sorted order must be known to pack the instances
                (prepare_sorted_draw, prepare_instance_infos)
                    .chain()
                    .in_set(RenderSet::Prepare),
            )
            .add_systems(Render, queue.in_set(RenderSet::QueueMeshes));
    }

    fn finish(&self, app: &mut App) {
        app.get_sub_app_mut(RenderApp)
            .unwrap()
            .init_resource::<SpineGPUPipeline>();
    }
}

fn extract_draw_phases(phases: Extract<Res<SpineDrawPhases>>, mut commands: Commands) {
    // Render world entities are cleared every frame
    let entities: Vec<_> = phases
        .enabled_phases()
        .map(|(index, z)| ExtractedDrawPhase { index, z })
        .collect();
    commands.spawn_batch(entities);
}

fn extract_draw_groups(draw_groups: Extract<Res<SkeletonDrawGroups>>, mut commands: Commands) {
    if draw_groups.is_changed() {
        commands.insert_resource(draw_groups.clone());
    }
}
