//! Parser for Spines atlas format.
//! The format is described in https://de.esotericsoftware.com/spine-atlas-format
//!
use std::collections::HashMap;

use anyhow::{Result, bail};
use bevy::{
    math::{DVec2, vec2},
    prelude::*,
    render::render_resource::ShaderType,
};

mod parser;
use bytemuck::Zeroable;
pub use parser::*;

// --------------------------------------------------------------------------------
// Public API
// --------------------------------------------------------------------------------

/// Texture atlas for a skeleton. Currently limited to exactly one page.
#[derive(Clone, Debug)]
pub struct Atlas {
    /// The texture
    texture: Handle<Image>,

    /// Atlas size in pixels. This may be wrong as we do some padding in preprocessing!
    size: Vec2,

    /// Position of regions in the atlas (in pixels)
    regions: Vec<Region>,

    /// uv bounds relative to original image (before putting into atlas)
    /// Needed for correct interpretation of UV values as UV in 0..1
    uv_bounds: Vec<(DVec2, DVec2)>,

    /// Map names to indices into regions and uv_bounds
    named_regions: HashMap<String, AtlasRegionIndex>,
}

/// Uniquely identifies an atlas region
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug, Zeroable)]
pub struct AtlasRegionIndex(pub(crate) u32);

// --------------------------------------------------------------------------------
// Crate internal
// --------------------------------------------------------------------------------

mod shader_types {
    #![allow(dead_code)] // encase 0.8.0 ShaderType derive emits unused `check` fns.

    use super::*;

    /// Which part of the Handle<Image> component to render (in pixels)
    #[derive(Debug, Component, Clone, Copy, ShaderType)]
    pub(crate) struct Region {
        pub(crate) min: Vec2,
        pub(crate) max: Vec2,
        // Where this image should be positioned. (0, 0)
        // is centered around the origin.
        pub(crate) offset: Vec2,
    }
}
pub(crate) use shader_types::Region;

impl Default for Atlas {
    fn default() -> Self {
        Self::new()
    }
}

impl Atlas {
    pub fn new() -> Self {
        Atlas {
            texture: Handle::Weak(AssetId::invalid()),
            regions: Vec::new(),
            named_regions: HashMap::new(),
            uv_bounds: Vec::new(),
            size: Vec2::ZERO,
        }
    }

    pub(crate) fn iter_regions(
        &self,
    ) -> impl Iterator<Item = (AtlasRegionIndex, String, Region)> + use<'_> {
        self.named_regions
            .iter()
            .map(|(name, idx)| (*idx, name.clone(), self.regions[idx.0 as usize]))
    }

    /// Parse the contents of an atlas file. Does no I/O: the texture is not set, instead the
    /// name of the page image (relative to the atlas file) is returned for the caller to load
    /// and pass to `set_texture`.
    pub fn from_string(data: &str) -> Result<(Self, String)> {
        let file = AtlasFile::from_string(data)?;

        if file.pages.len() != 1 {
            bail!("Expected exactly one page, got {}.", file.pages.len());
        }
        let page = &file.pages[0];

        let mut atlas = Self::new();

        atlas.size = page.size.as_vec2();

        for region in &page.regions {
            if region.rotate != Rotate::R0 {
                bail!("Region rotation is not supported. Got: {:?}", region);
            }

            let name = &region.name;

            let offsets = region.offsets.unwrap();
            let origin = region.origin.unwrap();

            let orig_size = offsets.zw().as_vec2();
            let size_in_atlas = region.bounds.zw().as_vec2();

            // Position in the atlas in pixels
            let min = region.bounds.xy().as_vec2();
            let max = min + size_in_atlas;

            // padding offset. uv 0,0 is here
            let offset_bl = offsets.xy().as_vec2();

            // Offset0 is measured from BOTTOM/left (*grr* esoteric),
            // but uv_min from TOP/left.

            let padding_tl = vec2(offset_bl.x, orig_size.y - offset_bl.y - size_in_atlas.y);

            let uv_min = padding_tl.as_dvec2() / orig_size.as_dvec2();
            // trimming to high coordinates is defined by size in atlas plus offset, uv 1,1 is here
            let uv_max = (padding_tl.as_dvec2() + size_in_atlas.as_dvec2()) / orig_size.as_dvec2();
            atlas.uv_bounds.push((uv_min, uv_max));

            trace!(
                "[{}] r={:?} min={:?} max={:?} offset0={:?} origsize={:?}",
                name, region, min, max, offset_bl, orig_size
            );

            // origin is measures shift to bottom-left,
            // whereas (0,0) means image is centered at (size.x/2,size.y/2) (assuming y is positive up)
            // we want shift to top-right with (0,0) meaning centered at (0,0)

            let origin_cropped =
                vec2(offsets.x as f32, offsets.y as f32) + size_in_atlas * vec2(0.5, 0.5) - origin;

            atlas.add_rect(name, min, max, origin_cropped);
        }

        Ok((atlas, page.name.clone()))
    }

    pub fn set_texture(&mut self, texture: Handle<Image>) {
        assert!(
            self.texture == Handle::Weak(AssetId::invalid()),
            "Should set texture only once"
        );
        self.texture = texture;
    }

    pub fn set_size(&mut self, size: Vec2) {
        self.size = size;
    }

    pub fn add_rect(
        &mut self,
        name: impl Into<String>,
        min: Vec2,
        max: Vec2,
        offset: Vec2,
    ) -> AtlasRegionIndex {
        self.regions.push(Region { min, max, offset });
        let sprite_index = AtlasRegionIndex(self.regions.len() as u32 - 1);
        self.named_regions.insert(name.into(), sprite_index);
        sprite_index
    }

    pub(crate) fn get(&self, name: &str) -> Option<Region> {
        let index = self.named_regions.get(name).copied()?;
        Some(self.regions[index.0 as usize])
    }

    pub(crate) fn get_texture(&self) -> Handle<Image> {
        self.texture.clone()
    }

    /// Spine atlas store UV relative to the original picture (of which the atlas may contain only a cropped region),
    /// we would like instead to know the UV relative to the atlas
    /// Return pair (scale, offset)
    /// new_uv = scale * uv + offset
    pub fn get_spine_uv_transform(&self, name: &str) -> Option<(DVec2, DVec2)> {
        let idx = self.named_regions.get(name).copied()?;
        // relative to original image
        let bounds = self.uv_bounds[idx.0 as usize];

        // UV position in the atlas
        let crop = self.regions[idx.0 as usize];
        let crop_min = crop.min.as_dvec2() / self.size.as_dvec2();
        let crop_max = crop.max.as_dvec2() / self.size.as_dvec2();

        let scale = (crop_max - crop_min) / (bounds.1 - bounds.0);
        let translation = -(((crop_max - crop_min) * bounds.0) / (bounds.1 - bounds.0)) + crop_min;

        Some((scale, translation))
    }

    /// Transform from region local UV coordinates to atlas UV coordinates
    /// Return pair (scale, offset)
    /// new_uv = scale * uv + offset
    pub fn get_region_uv_transform(&self, name: &str) -> Option<(DVec2, DVec2)> {
        let idx = self.named_regions.get(name).copied()?;

        // UV position in the atlas
        let crop = self.regions[idx.0 as usize];
        let crop_min = crop.min.as_dvec2() / self.size.as_dvec2();
        let crop_max = crop.max.as_dvec2() / self.size.as_dvec2();

        let scale = crop_max - crop_min;
        let translation = crop_min;

        Some((scale, translation))
    }
}
