use std::ops::Range;

use bevy::{
    ecs::system::{SystemParamItem, lifetimeless::SRes},
    render::{
        render_asset::{PrepareAssetError, RenderAsset, RenderAssets},
        render_resource::{
            BindGroup, BindGroupEntry, BindingResource, Buffer, BufferInitDescriptor, BufferUsages,
        },
        renderer::RenderDevice,
        texture::GpuImage,
    },
};
use bytemuck::cast_slice;

use crate::{SpineSkeleton, VertexInfo, render::SpineGPUPipeline};

/// GPU side of a [`SpineSkeleton`]: its vertex data, index buffer and atlas texture.
///
/// Prepared by Bevy's render asset machinery once the atlas texture is on the GPU, so a skeleton
/// is not drawn before that.
pub(crate) struct GpuSpineSkeleton {
    /// The indices (topology) of the vertices. Separate from the vertices as they get taken up
    /// by primitive assembly. Grouped by draw phase, in z order within each phase.
    pub(crate) index_buffer: Buffer,
    /// Total number of indices in `index_buffer`, 0 for a skeleton without any mesh
    pub(crate) index_count: u32,
    /// The indices of each draw phase in `index_buffer` (by phase, see `phase_range`)
    phase_ranges: Vec<Range<u32>>,
    /// Vertex buffer (`array<VertexInfo>`), atlas texture view and sampler, bind group 2
    pub(crate) bind_group: BindGroup,
}

impl RenderAsset for GpuSpineSkeleton {
    type SourceAsset = SpineSkeleton;
    type Param = (
        SRes<RenderDevice>,
        SRes<RenderAssets<GpuImage>>,
        SRes<SpineGPUPipeline>,
    );

    fn prepare_asset(
        skeleton: SpineSkeleton,
        (render_device, gpu_images, pipeline): &mut SystemParamItem<Self::Param>,
    ) -> Result<Self, PrepareAssetError<SpineSkeleton>> {
        let Some(gpu_image) = gpu_images.get(skeleton.texture().id()) else {
            return Err(PrepareAssetError::RetryNextUpdate(skeleton));
        };

        // wgpu does not accept empty buffers in bind groups, so a skeleton without vertices gets
        // one unused vertex (it has no indices to draw it with anyway).
        let unused_vertex = [VertexInfo::default()];
        let vertices = if skeleton.vertices.is_empty() {
            &unused_vertex[..]
        } else {
            skeleton.vertices.as_slice()
        };
        let vertex_buffer = render_device.create_buffer_with_data(&BufferInitDescriptor {
            label: Some("SpineSkeletonVertexBuffer"),
            usage: BufferUsages::STORAGE,
            contents: cast_slice(vertices),
        });

        // The concatenation of the per-slot index arrays, by phase and in z order
        let (indices, phase_ranges) = skeleton.skeleton.phase_indices();
        let index_count = indices.len() as u32;
        let unused_index = [0u32];
        let index_buffer = render_device.create_buffer_with_data(&BufferInitDescriptor {
            label: Some("SpineSkeletonIndexBuffer"),
            usage: BufferUsages::INDEX,
            contents: cast_slice(if indices.is_empty() {
                &unused_index[..]
            } else {
                indices.as_slice()
            }),
        });

        let bind_group = render_device.create_bind_group(
            "SpineSkeletonBindGroup",
            &pipeline.skeleton_layout,
            &[
                BindGroupEntry {
                    binding: 0,
                    resource: vertex_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::TextureView(&gpu_image.texture_view),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: BindingResource::Sampler(&gpu_image.sampler),
                },
            ],
        );

        Ok(Self {
            index_buffer,
            index_count,
            phase_ranges,
            bind_group,
        })
    }
}

impl GpuSpineSkeleton {
    /// The indices to draw in draw phase `phase`, empty if the skeleton has no slots in it.
    pub(crate) fn phase_range(&self, phase: usize) -> Range<u32> {
        self.phase_ranges.get(phase).cloned().unwrap_or(0..0)
    }
}
