use bevy::{prelude::*, render::mesh::VertexAttributeValues, render::render_resource::ShaderType};
use bytemuck::{Pod, Zeroable};

use crate::{
    ATTRIBUTE_ATTACHMENT_INDEX, ATTRIBUTE_BIND_POSITIONS_X, ATTRIBUTE_BIND_POSITIONS_Y,
    ATTRIBUTE_BONE_INDICES, ATTRIBUTE_BONE_WEIGHTS, ATTRIBUTE_SLOT_COLOR,
};

/// `VertexInfo::color` of an uncolored vertex.
const WHITE: u32 = 0xffff_ffff;

/// Limit of the number of vertices of a skeleton, checked when loading.
/// (The vertex buffers themselves are sized to the skeleton.)
pub(crate) const MAX_VERTICES: usize = 1024;

mod shader_types {
    #![allow(dead_code)] // encase 0.8.0 ShaderType derive emits unused `check` fns.

    use super::*;

    /// A single vertex of a skeleton type, the element of the vertex storage buffer
    /// (`array<VertexInfo>`). Keep in sync with spine_gpu.wgsl
    #[derive(Clone, Copy, Zeroable, Debug, ShaderType, Pod)]
    #[repr(C)]
    pub(crate) struct VertexInfo {
        pub(crate) bone_indices: UVec4,
        pub(crate) bone_weights: Vec4,
        pub(crate) bind_positions_x: Vec4,
        pub(crate) bind_positions_y: Vec4,
        // -----------------------------------------------------------
        pub(crate) uv: Vec2,
        /// Used to check visibility for any given instance
        pub(crate) attachment_index: u32,
        /// Straight-alpha RGBA, unorm8x4 with r in the low byte (`pack_4x8_unorm`): the slot
        /// setup color times the attachment color; white (`0xffffffff`) if neither is set.
        pub(crate) color: u32,
    }
}
pub(crate) use shader_types::*;

// The size of the buffer element, which the WGSL struct `VertexInfo` in spine_gpu.wgsl must
// match: four `vec4`s, then `vec2<f32>` and two `u32`s fill the last 16 bytes.
const _: () = assert!(size_of::<VertexInfo>() == 80);

/// Returns an error message if `total` vertices would exceed `MAX_VERTICES`.
pub(crate) fn check_vertex_count(total: usize) -> Result<(), String> {
    if total > MAX_VERTICES {
        Err(format!("{total} vertices (max {MAX_VERTICES})"))
    } else {
        Ok(())
    }
}

/// Append the vertices of `mesh` to `vertices`. Callers must ensure the capacity (see
/// `check_vertex_count`).
pub(crate) fn append_mesh_vertices(vertices: &mut Vec<VertexInfo>, mesh: &Mesh) {
    let Some(VertexAttributeValues::Uint32x4(bone_indices)) =
        mesh.attribute(ATTRIBUTE_BONE_INDICES)
    else {
        panic!();
    };
    let Some(VertexAttributeValues::Float32x4(bone_weights)) =
        mesh.attribute(ATTRIBUTE_BONE_WEIGHTS)
    else {
        panic!();
    };
    let Some(VertexAttributeValues::Float32x4(bind_positions_x)) =
        mesh.attribute(ATTRIBUTE_BIND_POSITIONS_X)
    else {
        panic!();
    };
    let Some(VertexAttributeValues::Float32x4(bind_positions_y)) =
        mesh.attribute(ATTRIBUTE_BIND_POSITIONS_Y)
    else {
        panic!();
    };
    let Some(VertexAttributeValues::Float32x2(uvs)) = mesh.attribute(Mesh::ATTRIBUTE_UV_0) else {
        panic!();
    };
    let Some(VertexAttributeValues::Uint32(attachment_indices)) =
        mesh.attribute(ATTRIBUTE_ATTACHMENT_INDEX)
    else {
        panic!();
    };

    // Optional: slot color x attachment color per vertex (default white)
    let color_values = match mesh.attribute(ATTRIBUTE_SLOT_COLOR) {
        Some(VertexAttributeValues::Uint32(values)) => Some(values),
        _ => None,
    };

    assert!(
        check_vertex_count(vertices.len() + bone_indices.len()).is_ok(),
        "Vertex overflow, call check_vertex_count first"
    );

    vertices.extend((0..bone_indices.len()).map(|idx| VertexInfo {
        bone_indices: UVec4::from_array(bone_indices[idx]),
        bone_weights: Vec4::from_array(bone_weights[idx]),
        bind_positions_x: Vec4::from_array(bind_positions_x[idx]),
        bind_positions_y: Vec4::from_array(bind_positions_y[idx]),
        uv: Vec2::from_array(uvs[idx]),
        attachment_index: attachment_indices[idx],
        color: color_values.map_or(WHITE, |v| v[idx]),
    }));
}

impl Default for VertexInfo {
    fn default() -> Self {
        Self {
            bone_indices: default(),
            // All bone weights being 0 indicates an unused slot
            bone_weights: Default::default(),
            bind_positions_x: Default::default(),
            bind_positions_y: Default::default(),
            uv: Default::default(),
            attachment_index: default(),
            color: WHITE,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertex_info_matches_wgsl_layout() {
        // The size WGSL gives `VertexInfo`, which is the stride of the storage buffer
        assert_eq!(
            VertexInfo::min_size().get() as usize,
            size_of::<VertexInfo>()
        );
        assert_eq!(std::mem::offset_of!(VertexInfo, uv), 64);
        assert_eq!(std::mem::offset_of!(VertexInfo, attachment_index), 72);
        assert_eq!(std::mem::offset_of!(VertexInfo, color), 76);
    }

    #[test]
    fn check_vertex_count_respects_max_vertices() {
        assert!(check_vertex_count(MAX_VERTICES).is_ok());
        let err = check_vertex_count(MAX_VERTICES + 1).unwrap_err();
        assert!(err.contains("1025") && err.contains("1024"), "{err}");
    }

    #[test]
    fn color_defaults_to_white_and_reaches_vertex_info() {
        use bevy::render::{mesh::PrimitiveTopology, render_asset::RenderAssetUsages};

        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        );
        mesh.insert_attribute(ATTRIBUTE_BONE_INDICES, vec![[0u32; 4]; 2]);
        mesh.insert_attribute(ATTRIBUTE_BONE_WEIGHTS, vec![[1.0f32, 0.0, 0.0, 0.0]; 2]);
        mesh.insert_attribute(ATTRIBUTE_BIND_POSITIONS_X, vec![[0.0f32; 4]; 2]);
        mesh.insert_attribute(ATTRIBUTE_BIND_POSITIONS_Y, vec![[0.0f32; 4]; 2]);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0f32; 2]; 2]);
        mesh.insert_attribute(ATTRIBUTE_ATTACHMENT_INDEX, vec![0u32; 2]);

        let mut vertices = Vec::new();
        append_mesh_vertices(&mut vertices, &mesh);
        assert_eq!(vertices[0].color, 0xffff_ffff);

        mesh.insert_attribute(ATTRIBUTE_SLOT_COLOR, vec![0x80ff_c030u32; 2]);
        append_mesh_vertices(&mut vertices, &mesh);
        assert_eq!(vertices[2].color, 0x80ff_c030);
    }
}
