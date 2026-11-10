use bevy::render::{mesh::MeshVertexAttribute, render_resource::VertexFormat};

pub const ATTRIBUTE_BONE_INDICES: MeshVertexAttribute =
    MeshVertexAttribute::new("BoneIndices", 0x9CC65CBB07C7CF93, VertexFormat::Uint32x4);

pub const ATTRIBUTE_BONE_WEIGHTS: MeshVertexAttribute =
    MeshVertexAttribute::new("BoneWeights", 0xECB00013922BB3AB, VertexFormat::Float32x4);

pub const ATTRIBUTE_BIND_POSITIONS_X: MeshVertexAttribute = MeshVertexAttribute::new(
    "BindPositionsX",
    0x7248F0F6813B61A2,
    VertexFormat::Float32x4,
);

pub const ATTRIBUTE_BIND_POSITIONS_Y: MeshVertexAttribute = MeshVertexAttribute::new(
    "BindPositionsY",
    0xF0753272E7B0CD54,
    VertexFormat::Float32x4,
);

pub const ATTRIBUTE_ATTACHMENT_INDEX: MeshVertexAttribute =
    MeshVertexAttribute::new("AttachmentIndex", 0x82A341F4B98C4820, VertexFormat::Uint32);

/// Straight-alpha RGBA color per vertex, 1 x u32 (`pack_4x8_unorm`, r in the low byte): slot
/// setup color times attachment color. Defaults to white (`0xffffffff`) when absent.
pub const ATTRIBUTE_SLOT_COLOR: MeshVertexAttribute =
    MeshVertexAttribute::new("SlotColor", 0x2F8A6D41B0C5E973, VertexFormat::Uint32);

pub(crate) fn pack_4x8_unorm(a: f32, b: f32, c: f32, d: f32) -> u32 {
    let a = (a.clamp(0.0, 1.0) * 255.0).round() as u32;
    let b = (b.clamp(0.0, 1.0) * 255.0).round() as u32;
    let c = (c.clamp(0.0, 1.0) * 255.0).round() as u32;
    let d = (d.clamp(0.0, 1.0) * 255.0).round() as u32;

    a | (b << 8) | (c << 16) | (d << 24)
}
