use std::f32::consts::PI;

use anyhow::{Result, anyhow};
use bevy::{
    math::{DVec2, dvec2, uvec4, vec2, vec3, vec4},
    prelude::*,
    render::{
        mesh::{Indices, PrimitiveTopology, VertexAttributeValues},
        render_asset::RenderAssetUsages,
    },
};

use crate::{
    ATTRIBUTE_ATTACHMENT_INDEX, ATTRIBUTE_BIND_POSITIONS_X, ATTRIBUTE_BIND_POSITIONS_Y,
    ATTRIBUTE_BONE_INDICES, ATTRIBUTE_BONE_WEIGHTS, ATTRIBUTE_SLOT_COLOR, BoneIndex, atlas::Region,
    pack_4x8_unorm,
};

use crate::loader::{AttachmentIndex, spine_json::Attachment};

/// Note: Spine actually pretends to have its rectangles of the original padded image size in some cases.
/// this will be refelected in Attachment::{width, height}.
/// this however makes it hard to map the attachment correctly from the UV atlas,
/// so we reduce the mesh to the cropped size. For this we modify width & height
/// of the (already copied) attachment before passing it on.
pub(super) fn crop_rectangle_to_region(attachment: &mut Attachment, region: &Region) {
    if let Attachment::Rectangle {
        width,
        height,
        x,
        y,
        scale_x,
        scale_y,
        rotation,
        ..
    } = attachment
    {
        // x, y in attachment are the center of the fully padded / original image relative to the
        // bone. We want instead the position of the center of the cropped image. The region offset
        // is in image space, so it is scaled and rotated like the image (see `make_mesh`) to get
        // the shift in bone space.
        let center_delta = region.offset * vec2(*scale_x, *scale_y);
        let center_delta = Vec2::from_angle(rotation.to_radians()).rotate(center_delta);
        *width = (region.max - region.min).x * *scale_x;
        *height = (region.max - region.min).y * *scale_y;
        *x += center_delta.x;
        *y += center_delta.y;
    }
}

/// Insert the color of the slot (slot color times attachment color, straight RGBA) as a vertex
/// attribute.
pub(super) fn insert_slot_color(mesh: &mut Mesh, color: [f32; 4]) {
    let packed = pack_4x8_unorm(color[0], color[1], color[2], color[3]);
    mesh.insert_attribute(ATTRIBUTE_SLOT_COLOR, vec![packed; mesh.count_vertices()]);
}

pub(super) fn make_mesh(
    attachment: &Attachment,
    bone_index: BoneIndex,
    spine_uv_transform: (DVec2, DVec2),
    region_uv_transform: (DVec2, DVec2),
    attachment_index: AttachmentIndex,
) -> Result<Mesh> {
    match attachment {
        Attachment::Rectangle {
            x,
            y,
            width,
            height,
            rotation,
            ..
        } => {
            let mesh = Mesh::from(Rectangle::from_size(vec2(*width, *height)))
                .rotated_by(Quat::from_rotation_z(rotation * PI / 180.0))
                .translated_by(vec3(*x, *y, 0.0));

            let positions = Vec::from(
                mesh.attribute(Mesh::ATTRIBUTE_POSITION)
                    .unwrap()
                    .as_float3()
                    .unwrap(),
            );

            let Some(VertexAttributeValues::Float32x2(orig_uvs)) =
                mesh.attribute(Mesh::ATTRIBUTE_UV_0)
            else {
                panic!();
            };

            // Note: In the mesh case below we get the UV coordinates from spine and they
            // will be relative to the original image before cropping & inclusion in the atlas.
            // Here however the UVs are all either 0 or 1 and refer to the region after inclusion in the atlas!

            let uvs = orig_uvs
                .iter()
                .map(|uv| {
                    (dvec2(uv[0] as f64, uv[1] as f64) * region_uv_transform.0
                        + region_uv_transform.1)
                        .as_vec2()
                })
                .collect::<Vec<_>>();

            Ok(mesh
                .with_inserted_attribute(
                    ATTRIBUTE_BIND_POSITIONS_X,
                    vec![
                        vec4(positions[0][0], 0.0, 0.0, 0.0),
                        vec4(positions[1][0], 0.0, 0.0, 0.0),
                        vec4(positions[2][0], 0.0, 0.0, 0.0),
                        vec4(positions[3][0], 0.0, 0.0, 0.0),
                    ],
                )
                .with_inserted_attribute(
                    ATTRIBUTE_BIND_POSITIONS_Y,
                    vec![
                        vec4(positions[0][1], 0.0, 0.0, 0.0),
                        vec4(positions[1][1], 0.0, 0.0, 0.0),
                        vec4(positions[2][1], 0.0, 0.0, 0.0),
                        vec4(positions[3][1], 0.0, 0.0, 0.0),
                    ],
                )
                .with_inserted_attribute(
                    ATTRIBUTE_BONE_INDICES,
                    vec![uvec4(bone_index.0, 0, 0, 0); 4],
                )
                .with_inserted_attribute(ATTRIBUTE_BONE_WEIGHTS, vec![vec4(1.0, 0.0, 0.0, 0.0); 4])
                .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
                .with_inserted_attribute(ATTRIBUTE_ATTACHMENT_INDEX, vec![attachment_index.0; 4]))
        }

        Attachment::Mesh {
            uvs,
            triangles,
            vertices,
            ..
        } => {
            let is_weighted = vertices.len() > uvs.len();

            let (bone_indices, bind_positions_x, bind_positions_y, bone_weights) = if is_weighted {
                read_vertices_weighted(vertices)?
            } else {
                read_vertices_unweighted(bone_index, vertices)?
            };

            let n = bone_indices.len();

            Ok(Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::default(),
            )
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, vec![Vec3::ZERO; n])
            .with_inserted_attribute(ATTRIBUTE_BONE_INDICES, bone_indices)
            .with_inserted_attribute(ATTRIBUTE_BIND_POSITIONS_X, bind_positions_x)
            .with_inserted_attribute(ATTRIBUTE_BIND_POSITIONS_Y, bind_positions_y)
            .with_inserted_attribute(ATTRIBUTE_BONE_WEIGHTS, bone_weights)
            .with_inserted_attribute(
                Mesh::ATTRIBUTE_UV_0,
                // Note: In spine JSON then UV coordinates refer to the original picture which our atlas may contain // a cropped version of
                // Here we scale UVs to the region as it is in our atlas as thats more useful to us
                // For better performance could also scale it directly to atlas UVs
                uvs.chunks(2)
                    .map(|uv| {
                        (dvec2(uv[0] as f64, uv[1] as f64) * spine_uv_transform.0
                            + spine_uv_transform.1)
                            .as_vec2()
                    })
                    .collect::<Vec<_>>(),
            )
            .with_inserted_attribute(ATTRIBUTE_ATTACHMENT_INDEX, vec![attachment_index.0; n])
            .with_inserted_indices(Indices::U32(triangles.clone())))
        }

        Attachment::LinkedMesh { .. } => Err(anyhow!(
            "linkedmesh attachments must be resolved before mesh creation"
        )),
    }
}

fn read_vertices_weighted(
    vertices: &[f32],
) -> Result<(Vec<UVec4>, Vec<Vec4>, Vec<Vec4>, Vec<Vec4>)> {
    let mut bone_indices = Vec::new();
    let mut bind_positions_x = Vec::new();
    let mut bind_positions_y = Vec::new();
    let mut bone_weights = Vec::new();

    let mut idx = 0;

    // weighted format goes like this:
    // [n_bones_vert0, idx0, x0, y0, weight0, idx1, x1, y1, weight1, ..., n_bones_vert1, ...]

    while idx < vertices.len() {
        let n_bones = vertices[idx] as usize;
        idx += 1;

        if n_bones > 4 {
            warn!(
                "Vertex #{} at pos {} is influenced by more than 4 bones: {:?}, will only honor the 4 biggest influences",
                bone_indices.len(),
                idx,
                n_bones
            );
        }

        let mut bone_idx = [0u32; 4];
        let mut x = [0.0; 4];
        let mut y = [0.0; 4];
        let mut weight = [0.0f32; 4];

        // Read from vertices[idx .. idx + 4] and write to bone_idx, x, y, weight [jj]
        // if there are more than 4 bone influences,

        for j in 0..n_bones {
            let w = vertices[idx + 3];
            let mut jj = j;

            if jj >= 4 {
                // Find smallest weight in `weight`
                let Some((jmin, &wmin)) = weight
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| a.total_cmp(b))
                else {
                    idx += 4;
                    continue;
                };
                // If that is smaller than `w`, overwrite it
                if wmin > w {
                    idx += 4;
                    continue;
                }
                jj = jmin;
            }

            bone_idx[jj] = vertices[idx] as u32;
            x[jj] = vertices[idx + 1];
            y[jj] = vertices[idx + 2];
            weight[jj] = w;
            idx += 4;
        }

        // weights may not be summing up to 1 in case we left some out, renormalize
        let sum = weight.iter().sum::<f32>();
        for w in weight.iter_mut() {
            *w /= sum;
        }

        bone_indices.push(UVec4::from_array(bone_idx));
        bind_positions_x.push(Vec4::from_array(x));
        bind_positions_y.push(Vec4::from_array(y));
        bone_weights.push(Vec4::from_array(weight));
    }

    Ok((
        bone_indices,
        bind_positions_x,
        bind_positions_y,
        bone_weights,
    ))
}

fn read_vertices_unweighted(
    parent_bone: BoneIndex,
    vertices: &[f32],
) -> Result<(Vec<UVec4>, Vec<Vec4>, Vec<Vec4>, Vec<Vec4>)> {
    let mut bone_indices = Vec::new();
    let mut bind_positions_x = Vec::new();
    let mut bind_positions_y = Vec::new();
    let mut bone_weights = Vec::new();

    for &[x, y] in vertices.as_chunks::<2>().0 {
        bone_indices.push(uvec4(parent_bone.0, 0, 0, 0));
        bind_positions_x.push(vec4(x, 0.0, 0.0, 0.0));
        bind_positions_y.push(vec4(y, 0.0, 0.0, 0.0));
        bone_weights.push(vec4(1.0, 0.0, 0.0, 0.0));
    }

    Ok((
        bone_indices,
        bind_positions_x,
        bind_positions_y,
        bone_weights,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cropped(rotation: f32, scale: Vec2) -> (f32, f32, f32, f32) {
        let mut attachment = Attachment::new_rectangle(10.0, 20.0, 100.0, 50.0);
        if let Attachment::Rectangle {
            rotation: r,
            scale_x,
            scale_y,
            ..
        } = &mut attachment
        {
            *r = rotation;
            *scale_x = scale.x;
            *scale_y = scale.y;
        }
        // A 40x50 region whose center is 30 px right of the original image's center.
        let region = Region {
            min: vec2(0.0, 0.0),
            max: vec2(40.0, 50.0),
            offset: vec2(30.0, 0.0),
        };
        crop_rectangle_to_region(&mut attachment, &region);
        let Attachment::Rectangle {
            x,
            y,
            width,
            height,
            ..
        } = attachment
        else {
            unreachable!()
        };
        (x, y, width, height)
    }

    fn assert_close(actual: (f32, f32, f32, f32), expected: (f32, f32, f32, f32)) {
        let a = [actual.0, actual.1, actual.2, actual.3];
        let e = [expected.0, expected.1, expected.2, expected.3];
        for (a, e) in a.iter().zip(e) {
            assert!((a - e).abs() < 1e-3, "{actual:?} != {expected:?}");
        }
    }

    #[test]
    fn crop_shifts_center_by_region_offset() {
        assert_close(cropped(0.0, Vec2::ONE), (40.0, 20.0, 40.0, 50.0));
    }

    #[test]
    fn crop_offset_follows_attachment_rotation() {
        // Rotated by 90 degrees, the image's x axis points along the bone's y axis.
        assert_close(cropped(90.0, Vec2::ONE), (10.0, 50.0, 40.0, 50.0));
        assert_close(cropped(-90.0, Vec2::ONE), (10.0, -10.0, 40.0, 50.0));
    }

    #[test]
    fn crop_offset_is_scaled_before_rotation() {
        assert_close(cropped(90.0, vec2(2.0, 1.0)), (10.0, 80.0, 80.0, 50.0));
    }
}
