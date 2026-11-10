//! Parse a subset of Spine2ds atlas definition
//! See https://esotericsoftware.com/spine-atlas-format

use std::{error::Error as StdError, str::FromStr};

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use bevy::{math::uvec4, prelude::*};

/// Split a `key: value,value,...` line of the atlas format into its key and the rest of the line.
/// Keys consist of word characters; anything else is not a key-value line (it names a region).
fn split_key_value(line: &str) -> Option<(&str, &str)> {
    let (key, value) = line.split_once(':')?;
    let is_key = !key.is_empty() && key.chars().all(|c| c.is_alphanumeric() || c == '_');
    is_key.then_some((key, value))
}

// --------------------------------------------------------------------------------
// Public API
// --------------------------------------------------------------------------------

#[derive(Debug)]
pub struct AtlasFile {
    pub(crate) pages: Vec<Page>,
}

// --------------------------------------------------------------------------------
// Crate internal
// --------------------------------------------------------------------------------

#[derive(Default, Debug)]
pub(crate) struct Page {
    pub(crate) name: String,
    pub(crate) size: UVec2,
    pub(crate) format: String,
    pub(crate) filter: Vec<String>,
    pub(crate) repeat: String,
    pub(crate) pma: bool,
    pub(crate) regions: Vec<Region>,
}

#[derive(Default, Debug)]
pub(crate) struct Region {
    pub(crate) name: String,
    pub(crate) index: i32,
    pub(crate) bounds: UVec4,
    // We may only store a cropped version of an original image here.
    // In that case `offsets` tells us abouth the original image as
    // UV coordinates may refer to that.
    // offsets: left, top, originalWidth, originalHeight.
    // Note: If original size is different from size in the atlas it means it was cropped also to the top/right, *not* that it was scaled!
    pub(crate) offsets: Option<UVec4>,

    pub(crate) rotate: Rotate,

    // left, right, top, bottom splits for ninepatch
    pub(crate) split: Option<UVec4>,

    // left, right, top, bottom padding for ninepatch
    pub(crate) pad: Option<IVec4>,

    // Not documented by esoteric
    // Experimentation shows this seems to be the pivot relative to the *bottom left* corner of the padded image
    // i.e. If a 500x500 image is centered, this is 250,250.
    // If it is moved by 100,200 from the center into the top/right quadrant, this becomes 150,50
    // Exact (not rounded): the default is half the original size, which may be odd.
    pub(crate) origin: Option<Vec2>,
}

// Rotation about 90 degrees means (u', v') := (1 - v, u)
#[derive(Default, Debug, PartialEq, Eq)]
pub(crate) enum Rotate {
    #[default]
    R0,
    R90,
    R180,
    R270,
}

impl Region {
    pub(crate) fn with_defaults(mut self) -> Self {
        self.offsets
            .get_or_insert(uvec4(0, 0, self.bounds.z, self.bounds.w));
        self.origin
            .get_or_insert(self.offsets.unwrap().zw().as_vec2() / 2.0);
        self
    }
}

impl AtlasFile {
    pub fn from_string(data: &str) -> Result<AtlasFile> {
        parse_atlas(data.lines())
    }
}

fn parse_atlas<'a>(mut data: impl Iterator<Item = &'a str> + Clone) -> Result<AtlasFile> {
    let mut pages = Vec::new();

    while let Some(name) = data.next() {
        let mut page = Page {
            name: name.to_string(),
            ..default()
        };
        parse_page(&mut data, &mut page).with_context(|| format!("Parsing page '{}'", name))?;
        pages.push(page);
    }

    Ok(AtlasFile { pages })
}

fn parse_page<'a>(data: &mut impl Iterator<Item = &'a str>, page: &mut Page) -> Result<()> {
    while let Some(line) = data.next() {
        if line.is_empty() {
            break;
        }

        if let Some((key, values)) = split_key_value(line) {
            let mut values = values.split(',');

            match key {
                "size" => {
                    page.size.x = iter_parse(&mut values)?;
                    page.size.y = iter_parse(&mut values)?;
                }
                "format" => {
                    page.format = values.next().ok_or(anyhow!("empty format value"))?.into();
                }
                "filter" => {
                    page.filter.extend(values.map(|v| v.to_string()));
                }
                "repeat" => {
                    page.repeat = values.next().ok_or(anyhow!("empty repeat value"))?.into();
                }
                "pma" => {
                    page.pma = iter_parse(&mut values)?;
                }
                _ => bail!("Unexpected key: '{}'", key),
            }
        } else {
            page.regions
                .extend(parse_regions(line.to_string(), data).context("Parsing regions")?);
        };
    }

    Ok(())
}

fn parse_regions<'a>(
    first_line: String,
    data: &mut impl Iterator<Item = &'a str>,
) -> Result<Vec<Region>> {
    let mut region = Region {
        name: first_line,
        ..default()
    };

    let mut regions = Vec::new();

    for line in data.by_ref() {
        if line.is_empty() {
            regions.push(region.with_defaults());
            return Ok(regions);
        }

        if let Some((key, values)) = split_key_value(line) {
            let mut values = values.split(',');

            match key {
                "index" => {
                    region.index = iter_parse(&mut values)?;
                }
                "bounds" => {
                    region.bounds.x = iter_parse(&mut values)?;
                    region.bounds.y = iter_parse(&mut values)?;
                    region.bounds.z = iter_parse(&mut values)?;
                    region.bounds.w = iter_parse(&mut values)?;
                }
                "offsets" => {
                    region.offsets = Some(UVec4::new(
                        iter_parse(&mut values)?,
                        iter_parse(&mut values)?,
                        iter_parse(&mut values)?,
                        iter_parse(&mut values)?,
                    ));
                }
                "rotate" => {
                    let v = values.next().ok_or(anyhow!("missing rotation value"))?;
                    ensure!(values.next().is_none(), "not exactly one value");
                    region.rotate = match v {
                        "0" => Rotate::R0,
                        "true" | "90" => Rotate::R90,
                        "180" => Rotate::R180,
                        "270" => Rotate::R270,
                        _ => bail!("Unexpected rotation value: '{}'", v),
                    }
                }
                "split" => {
                    region.split = Some(UVec4::new(
                        iter_parse(&mut values)?,
                        iter_parse(&mut values)?,
                        iter_parse(&mut values)?,
                        iter_parse(&mut values)?,
                    ));
                }
                "pad" => {
                    region.pad = Some(IVec4::new(
                        iter_parse(&mut values)?,
                        iter_parse(&mut values)?,
                        iter_parse(&mut values)?,
                        iter_parse(&mut values)?,
                    ));
                }
                "origin" => {
                    region.origin = Some(Vec2::new(
                        iter_parse(&mut values)?,
                        iter_parse(&mut values)?,
                    ));
                }
                _ => bail!("Unexpected key: '{}'", key),
            }
        } else {
            regions.push(region.with_defaults());
            region = Region {
                name: line.to_string(),
                ..default()
            };
        }
    }

    regions.push(region.with_defaults());
    Ok(regions)
}

fn iter_parse<'a, F: FromStr>(values: &mut impl Iterator<Item = &'a str>) -> Result<F>
where
    <F as FromStr>::Err: StdError + Send + Sync + 'static,
{
    let v = values.next().ok_or(anyhow!("too few values"))?;
    let r = v.parse().with_context(|| format!("Parsing '{}'", v))?;
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_value_lines_have_a_word_key() {
        assert_eq!(split_key_value("size:180,268"), Some(("size", "180,268")));
        assert_eq!(split_key_value("pma:true"), Some(("pma", "true")));
        assert_eq!(split_key_value("bounds:"), Some(("bounds", "")));
    }

    #[test]
    fn other_lines_name_regions() {
        assert_eq!(split_key_value("Arm L"), None);
        assert_eq!(split_key_value(""), None);
        assert_eq!(split_key_value(":x"), None);
        // The key must be a single word
        assert_eq!(split_key_value("my region:1"), None);
    }

    #[test]
    fn rotate_takes_exactly_one_value() {
        let atlas = |rotate: &str| {
            AtlasFile::from_string(&format!(
                "p.png
r
rotate:{rotate}
"
            ))
        };
        assert_eq!(atlas("90").unwrap().pages[0].regions[0].rotate, Rotate::R90);
        assert!(atlas("90,180").is_err());
        assert!(atlas("45").is_err());
    }
}
