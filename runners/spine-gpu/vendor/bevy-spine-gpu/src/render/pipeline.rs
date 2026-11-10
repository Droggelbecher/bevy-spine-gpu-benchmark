use bevy::{
    prelude::*,
    render::{
        render_resource::{
            BindGroupLayout, BindGroupLayoutEntry, BindingType, BlendState, BufferBindingType,
            ColorTargetState, ColorWrites, FragmentState, FrontFace, MultisampleState, PolygonMode,
            PrimitiveState, RenderPipelineDescriptor, SamplerBindingType, ShaderStages,
            SpecializedRenderPipeline, TextureFormat, TextureSampleType, TextureViewDimension,
            VertexState,
        },
        renderer::RenderDevice,
        texture::BevyDefault as _,
        view::ViewTarget,
    },
    sprite::{Mesh2dPipeline, Mesh2dPipelineKey},
};

pub(crate) const SPINE_GPU_SHADER_HANDLE: Handle<Shader> =
    Handle::weak_from_u128(0x926214D0445708DBF7980BA6AFB73161);

#[derive(Resource)]
pub(crate) struct SpineGPUPipeline {
    pub(crate) shader: Handle<Shader>,
    pub(crate) mesh2d_pipeline: Mesh2dPipeline,
    pub(crate) instance_infos_layout: BindGroupLayout,
    pub(crate) skeleton_layout: BindGroupLayout,
}

impl FromWorld for SpineGPUPipeline {
    fn from_world(world: &mut World) -> Self {
        let mesh_pipeline = world.resource::<Mesh2dPipeline>().clone();
        let render_device = world.resource::<RenderDevice>();

        Self {
            shader: SPINE_GPU_SHADER_HANDLE,
            mesh2d_pipeline: mesh_pipeline,
            instance_infos_layout: render_device.create_bind_group_layout(
                "InstanceInfos",
                &[
                    // Instance headers
                    BindGroupLayoutEntry {
                        binding: 0,
                        visibility: ShaderStages::VERTEX_FRAGMENT,
                        ty: BindingType::Buffer {
                            ty: BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    // Bones of all instances
                    BindGroupLayoutEntry {
                        binding: 1,
                        visibility: ShaderStages::VERTEX_FRAGMENT,
                        ty: BindingType::Buffer {
                            ty: BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            ),
            skeleton_layout: render_device.create_bind_group_layout(
                "SpineSkeleton",
                &[
                    BindGroupLayoutEntry {
                        binding: 0,
                        visibility: ShaderStages::VERTEX_FRAGMENT,
                        ty: BindingType::Buffer {
                            ty: BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    BindGroupLayoutEntry {
                        binding: 1,
                        visibility: ShaderStages::VERTEX_FRAGMENT,
                        ty: BindingType::Texture {
                            sample_type: TextureSampleType::Float { filterable: true },
                            view_dimension: TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    BindGroupLayoutEntry {
                        binding: 2,
                        visibility: ShaderStages::VERTEX_FRAGMENT,
                        ty: BindingType::Sampler(SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            ),
        }
    }
}

impl SpecializedRenderPipeline for SpineGPUPipeline {
    type Key = Mesh2dPipelineKey;

    fn specialize(
        &self,
        key: Self::Key,
    ) -> bevy::render::render_resource::RenderPipelineDescriptor {
        RenderPipelineDescriptor {
            vertex: VertexState {
                shader: self.shader.clone(),
                entry_point: "vertex".into(),
                shader_defs: vec![],
                buffers: vec![], //vertex_layout],
            },
            fragment: Some(FragmentState {
                shader: self.shader.clone(),
                entry_point: "fragment".into(),
                shader_defs: vec![],
                targets: vec![Some(ColorTargetState {
                    format: match key.contains(Mesh2dPipelineKey::HDR) {
                        true => ViewTarget::TEXTURE_FORMAT_HDR,
                        false => TextureFormat::bevy_default(),
                    },
                    blend: Some(BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: ColorWrites::ALL,
                })],
            }),
            label: Some("spinegpu_pipeline".into()),
            layout: vec![
                // Bind groups 0 & 1 are bevy default
                // (0) Camera/view data
                self.mesh2d_pipeline.view_layout.clone(),
                // (1) Basicaly the global transform of the mesh. We ignore this as position is calculated from bones
                // self.mesh2d_pipeline.mesh_layout.clone(),
                // (1) Instance headers and bones, will be updated every frame
                self.instance_infos_layout.clone(),
                // (2) Vertices and atlas texture of the skeleton type
                self.skeleton_layout.clone(),
            ],
            push_constant_ranges: vec![],
            primitive: PrimitiveState {
                front_face: FrontFace::Ccw,
                // Spine triangles *seem* to be clockwise, bevy meshes we generate for region attachments
                // are CCW. Clockwise-ness of spine triangles is however not documented, hard to measure and may change during animation
                // (due to bone influences), so we just dont cull anything here
                cull_mode: None, //Some(Face::Back),
                unclipped_depth: false,
                polygon_mode: PolygonMode::Fill,
                conservative: false,
                topology: key.primitive_topology(),
                strip_index_format: None,
            },
            depth_stencil: None,
            multisample: MultisampleState {
                count: key.msaa_samples(),
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
        }
    }
}
