#import bevy_sprite::mesh2d_view_bindings::view

struct VertexInput {
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    // Constant per vertex/slot: premultiplied tint, the vertex color (slot x attachment) times
    // the instance color.
    @location(1) @interpolate(flat) tint: vec4<f32>,
}

// Keep in sync with vertex_info.rs. 80 bytes.
struct VertexInfo {
    bone_indices: vec4<u32>,
    bone_weights: vec4<f32>,
    bind_positions_x: vec4<f32>,
    bind_positions_y: vec4<f32>,

    uv: vec2<f32>,
    // Used to check visibility for any given instance
    attachment_index: u32,
    // Straight-alpha RGBA, unorm8x4 with r in the low byte
    color: u32,
}

// The header of an instance, keep in sync with instance_infos.rs. 32 bytes.
struct InstanceInfo {
    // Bitmask of currently visible attachments (atlas region indices)
    attachments: vec3<u32>,

    // Index of the first bone of this instance in `bones`
    bone_offset: u32,

    color: vec4<f32>,
}

// Keep in sync with GlobalBoneTransform in instance_infos.rs. 32 bytes.
struct BoneTransform {
    scale: mat2x2<f32>,
    translation: vec2<f32>,
    _pad: vec2<f32>
}

// Instance = Skeleton
// Vertices = All vertices of all the meshes in one skeleton

// Will be updated every frame
// index: Skeleton instance (including first_instance of the draw call)
@group(1) @binding(0) var<storage, read> instance_infos: array<InstanceInfo>;

// The current bone transforms of all instances, each instance has the bones of its skeleton type
// starting at `InstanceInfo.bone_offset`
@group(1) @binding(1) var<storage, read> bones: array<BoneTransform>;

// The vertices of the skeleton type being drawn, constant after loading
// index: vertex index of the draw call
@group(2) @binding(0) var<storage, read> vertices: array<VertexInfo>;

@group(2) @binding(1) var texture: texture_2d<f32>;
@group(2) @binding(2) var sampl: sampler;

@vertex
fn vertex(input: VertexInput) -> VertexOutput {
    // Pointers: never copy the (large) structs out of the storage buffers.
    let instance = &instance_infos[input.instance_index];
    let vert = &vertices[input.vertex_index];

    var output: VertexOutput;

    // Hidden attachment: all its vertices go to the same point outside of the clip volume,
    // so its triangles have no area and are clipped.
    if uvec3_get_bit((*instance).attachments, (*vert).attachment_index) == 0u {
        output.clip_position = vec4(2.0, 2.0, 2.0, 1.0);
        return output;
    }

    var skinned_position: vec2<f32> = vec2<f32>(0.0);
    for (var i = 0u; i < 4u; i += 1u) {
        let bone_index = (*vert).bone_indices[i];
        let weight = (*vert).bone_weights[i];
        let px = (*vert).bind_positions_x[i];
        let py = (*vert).bind_positions_y[i];

        let bone = &bones[(*instance).bone_offset + bone_index];

        let transformed = (*bone).scale * vec2(px, py) + (*bone).translation;
        skinned_position += transformed * weight;
    }

    output.clip_position = view.clip_from_world * vec4(skinned_position, 0.0, 1.0);
    output.uv = (*vert).uv;

    // Output is premultiplied alpha: premultiply the tint itself.
    // Components > 1.0 of the instance color brighten (HDR).
    let color = unpack4x8unorm((*vert).color) * (*instance).color;
    output.tint = vec4(color.rgb * color.a, color.a);
    return output;
}

@fragment
fn fragment(input: VertexOutput) -> @location(0) vec4<f32> {
    // The texture is premultiplied alpha
    return textureSample(texture, sampl, input.uv) * input.tint;
}

// Supports 96 attachments (3 * 32 bits).
// Keep in sync with MAX_ATTACHMENTS_PER_SKELETON in instance_infos.rs
fn uvec3_get_bit(v: vec3<u32>, idx: u32) -> u32 {
    if idx < 32 {
        return (v.x & (1u << idx));
    }
    else if idx < 2 * 32 {
        return (v.y & (1u << (idx - 32)));
    }
    else {
        return (v.z & (1u << (idx - 2*32)));
    }
}
