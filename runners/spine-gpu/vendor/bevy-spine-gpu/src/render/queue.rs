use bevy::{
    core_pipeline::core_2d::Transparent2d,
    math::FloatOrd,
    prelude::*,
    render::{
        mesh::PrimitiveTopology,
        render_phase::{DrawFunctions, PhaseItemExtraIndex, ViewSortedRenderPhases},
        render_resource::{PipelineCache, SpecializedRenderPipelines},
        view::ExtractedView,
    },
    sprite::Mesh2dPipelineKey,
};

use crate::render::{ExtractedDrawPhase, draw::DrawSpineGpu, pipeline::SpineGPUPipeline};

#[allow(clippy::too_many_arguments)]
pub(super) fn queue(
    transparent_draw_functions: Res<DrawFunctions<Transparent2d>>,
    spinegpu_pipeline: Res<SpineGPUPipeline>,
    mut pipelines: ResMut<SpecializedRenderPipelines<SpineGPUPipeline>>,
    pipeline_cache: Res<PipelineCache>,
    msaa: Res<Msaa>,
    mut transparent_render_phases: ResMut<ViewSortedRenderPhases<Transparent2d>>,
    views: Query<(Entity, &ExtractedView)>,
    phases: Query<(Entity, &ExtractedDrawPhase)>,
) {
    // Cameras
    for (view_entity, view) in &views {
        let Some(transparent_phase) = transparent_render_phases.get_mut(&view_entity) else {
            continue;
        };

        let draw_function = transparent_draw_functions.read().id::<DrawSpineGpu>();

        let mesh_key = Mesh2dPipelineKey::from_msaa_samples(msaa.samples())
            | Mesh2dPipelineKey::from_hdr(view.hdr)
            | Mesh2dPipelineKey::from_primitive_topology(PrimitiveTopology::TriangleList);

        let pipeline = pipelines.specialize(&pipeline_cache, &spinegpu_pipeline, mesh_key);

        // One item per enabled draw phase, each draws the slots of its phase of all skeletons
        for (entity, phase) in &phases {
            transparent_phase.add(Transparent2d {
                sort_key: FloatOrd(phase.z),
                // The draw command reads the phase from it
                entity,
                pipeline,
                draw_function,
                batch_range: 0..1,
                extra_index: PhaseItemExtraIndex::NONE,
            });
        }
    }
}
