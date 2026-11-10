use bevy::{
    ecs::{
        query::ROQueryItem,
        system::{
            SystemParamItem,
            lifetimeless::{Read, SRes},
        },
    },
    prelude::*,
    render::{
        render_asset::RenderAssets,
        render_phase::{
            PhaseItem, RenderCommand, RenderCommandResult, SetItemPipeline, TrackedRenderPass,
        },
        render_resource::IndexFormat,
    },
    sprite::SetMesh2dViewBindGroup,
};

use crate::render::{
    ExtractedDrawPhase, GpuSpineSkeleton, InstanceInfosBuffers, InstanceLayout,
    draw_order::SortedDraw,
};

pub(crate) type DrawSpineGpu = (
    // Standard Bevy stuff
    SetItemPipeline,
    SetMesh2dViewBindGroup<0>,
    DrawInstanceInfoAndSkeleton,
);

pub(crate) struct DrawInstanceInfoAndSkeleton;

impl<P: PhaseItem> RenderCommand<P> for DrawInstanceInfoAndSkeleton {
    type Param = (
        SRes<RenderAssets<GpuSpineSkeleton>>,
        SRes<InstanceLayout>,
        Option<SRes<InstanceInfosBuffers>>,
        SRes<SortedDraw>,
    );
    type ViewQuery = ();
    type ItemQuery = Read<ExtractedDrawPhase>;

    #[inline]
    fn render<'w>(
        _item: &P,
        _view: (),
        draw_phase: Option<ROQueryItem<'w, Self::ItemQuery>>,
        (gpu_skeletons, layout, instance_info_buffers, sorted_draw): SystemParamItem<
            'w,
            '_,
            Self::Param,
        >,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some(instance_info_buffers) = instance_info_buffers else {
            warn!("instance infos buffer not available");
            return RenderCommandResult::Failure;
        };

        let Some(draw_phase) = draw_phase else {
            return RenderCommandResult::Failure;
        };

        let gpu_skeletons = gpu_skeletons.into_inner();

        // Header and bone buffers of all instances, one bind group for the whole pass. It does not
        // exist before the first instances are prepared, there is nothing to draw then.
        let Some(instances_bind_group) = &instance_info_buffers.into_inner().bind_group else {
            return RenderCommandResult::Success;
        };
        pass.set_bind_group(1, instances_bind_group, &[]);

        // Iterate over the static skeleton types, in draw order.
        // Each has its own topology (indices) and bind group (vertices and atlas texture).
        // Types that are not prepared yet (e.g. their texture is still loading) are
        // skipped, their instances show up once they are.
        // Only the indices of this item's draw phase are drawn. Every phase is its own phase
        // item, so the phases of all instances end up in separate layers.

        for draw in &layout.static_draws {
            let Some(gpu_skeleton) = gpu_skeletons.get(draw.skeleton) else {
                continue;
            };

            let indices = gpu_skeleton.phase_range(draw_phase.index);
            if indices.is_empty() {
                // No mesh in this phase; wgpu rejects empty buffer slices
                continue;
            }

            pass.set_bind_group(2, &gpu_skeleton.bind_group, &[]);
            pass.set_index_buffer(gpu_skeleton.index_buffer.slice(..), 0, IndexFormat::Uint32);

            // One instanced draw per skeleton type. The phase's index range holds all its slots
            // of the skeleton concatenated in z order, so primitives are rasterized in order
            // instance-major, then index order: all slots of instance 0 (back to front), then
            // all slots of instance 1, and so on. That is exactly painter's order, so
            // overlapping instances render correctly without a draw call per instance and slot.
            // The order *between* instances is the instance buffer order (spawn order, with
            // swap-remove on despawn). Types that need a per-instance order are in the sorted
            // group, drawn below.
            // The instances of the type are `draw.first..draw.first + draw.len` in the global
            // header buffer; `first_instance` is part of the shader's `instance_index`.
            pass.draw_indexed(indices, 0, draw.first..draw.first + draw.len);
        }

        // The sorted group is drawn after (i.e. in front of) all static types.
        // Its instances are sorted across types, so every run of consecutive instances of one
        // type needs its own draw, with the bind group and index buffer of that type.
        // The instances are packed in draw order after the static ones, `run.first` selects
        // the instances of the run among them.
        for run in &sorted_draw.runs {
            let Some(gpu_skeleton) = gpu_skeletons.get(run.skeleton) else {
                continue;
            };
            let indices = gpu_skeleton.phase_range(draw_phase.index);
            if indices.is_empty() {
                continue;
            }

            let first = layout.sorted_first + run.first;
            pass.set_bind_group(2, &gpu_skeleton.bind_group, &[]);
            pass.set_index_buffer(gpu_skeleton.index_buffer.slice(..), 0, IndexFormat::Uint32);
            pass.draw_indexed(indices, 0, first..first + run.len);
        }

        RenderCommandResult::Success
    }
}
