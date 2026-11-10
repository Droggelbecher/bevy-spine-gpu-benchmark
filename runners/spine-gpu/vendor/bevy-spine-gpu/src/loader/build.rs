use std::collections::HashMap;

use anyhow::{Result, anyhow};
use bevy::prelude::*;

use crate::{
    Animation, Skeleton, SkeletonInfo, SpineSkeleton, VertexInfo, append_mesh_vertices,
    check_vertex_count,
};

mod animation;
mod atlas;
mod image;
mod mesh;
mod spine;

pub(crate) use atlas::make_atlas_skeleton;
pub(crate) use image::make_image_skeleton;
#[cfg(test)]
pub(crate) use spine::SpineFile;
pub(crate) use spine::parse_spine_file;

/// A skeleton type with its animations, built but not yet an asset.
pub(super) struct SkeletonData {
    pub(super) skeleton: Skeleton,
    /// Vertex data of the meshes, in the order their topology was added to `skeleton`.
    pub(super) vertices: Vec<VertexInfo>,
    pub(super) animations: HashMap<String, Animation>,
    /// Name lookups; the animation durations are filled in by `into_asset`.
    pub(super) info: SkeletonInfo,
}

impl SkeletonData {
    pub(super) fn new(skeleton: Skeleton) -> Self {
        Self {
            skeleton,
            vertices: Vec::new(),
            animations: HashMap::new(),
            info: SkeletonInfo::default(),
        }
    }

    /// Add the mesh's topology to the skeleton (drawn in draw phase `phase`, at `z` within it)
    /// and keep its vertex data for the upload.
    pub(super) fn add_mesh(&mut self, mesh: Mesh, phase: usize, z: u32, path: &str) -> Result<()> {
        let total = self.vertices.len() + mesh.count_vertices();
        check_vertex_count(total)
            .map_err(|e| anyhow!("Too many vertices for skeleton {:?}: {}", path, e))?;
        // Create index buffer for attachment
        self.skeleton.add_mesh(&mesh, phase, z);
        append_mesh_vertices(&mut self.vertices, &mesh);
        Ok(())
    }

    pub(crate) fn into_asset(self) -> SpineSkeleton {
        let Self {
            skeleton,
            vertices,
            animations,
            mut info,
        } = self;
        info.animation_durations = animation_durations(&animations);
        SpineSkeleton {
            skeleton,
            vertices,
            animations,
            info,
        }
    }
}

pub(super) fn animation_durations(animations: &HashMap<String, Animation>) -> HashMap<String, f32> {
    animations
        .iter()
        .map(|(label, animation)| (label.clone(), animation.duration))
        .collect()
}
