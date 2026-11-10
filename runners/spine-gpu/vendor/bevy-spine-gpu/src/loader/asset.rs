//! Asset loaders for [`SpineSkeleton`]s.
//!
//! Both loaders only read through the asset server (the main file via the `Reader`, siblings via
//! `LoadContext::read_asset_bytes`, the texture as a nested load), so they work with any asset
//! source, including embedded assets. They are thin wrappers around the pure build functions.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context as _, Result};
use bevy::{
    asset::{AssetLoader, AsyncReadExt as _, LoadContext, io::Reader},
    prelude::*,
    render::texture::{ImageLoaderSettings, ImageSampler},
};

use crate::{Atlas, SpineSkeleton};

use super::{
    SlotRule,
    build::{make_atlas_skeleton, parse_spine_file},
};

/// Prefix of the labels of the skins of a `.json` skeleton, e.g. `Ella.json#skin/Ella`.
pub(super) const SKIN_LABEL_PREFIX: &str = "skin/";

/// Loads a Spine `.json` with the `.atlas` of the same name next to it.
///
/// The asset is the skeleton with the skin selected for `None` (see `LoadSpine`). If the file has
/// more than one skin, each of them is additionally available as the labeled sub-asset
/// `skin/<name>`.
#[derive(Default)]
pub(crate) struct SpineJsonLoader {
    /// Draw phases of slots by name, applied to every loaded file (`SpineGPUPlugin::slot_rules`)
    pub(crate) slot_rules: Arc<[SlotRule]>,
}

/// Loads a Spine `.atlas` as a skeleton with one static animation per region.
#[derive(Default)]
pub(crate) struct SpineAtlasLoader;

impl AssetLoader for SpineJsonLoader {
    type Asset = SpineSkeleton;
    type Settings = ();
    type Error = anyhow::Error;

    async fn load<'a>(
        &'a self,
        reader: &'a mut Reader<'_>,
        _settings: &'a (),
        load_context: &'a mut LoadContext<'_>,
    ) -> Result<SpineSkeleton> {
        let path = load_context.path().to_path_buf();
        let path_str = path.display().to_string();

        let json = read_string(reader).await?;
        let atlas_path = path.with_extension("atlas");
        let atlas_data = read_sibling(load_context, atlas_path.clone())
            .await
            .with_context(|| format!("When reading the atlas of {path_str:?}"))?;

        let file = parse_spine_file(&path_str, &json)?.with_slot_rules(self.slot_rules.clone());
        let atlas = parse_atlas(load_context, &atlas_path, &atlas_data)
            .with_context(|| format!("When loading atlas {:?}", atlas_path.display()))?;

        let skeleton = file.build(None, atlas.clone())?.into_asset();
        // A file with a single skin is the skin
        let skins: Vec<_> = file.skin_names().collect();
        if skins.len() > 1 {
            for skin in skins {
                let skeleton = file.build(Some(skin), atlas.clone())?.into_asset();
                load_context.add_labeled_asset(format!("{SKIN_LABEL_PREFIX}{skin}"), skeleton);
            }
        }
        Ok(skeleton)
    }

    fn extensions(&self) -> &[&str] {
        &["json"]
    }
}

impl AssetLoader for SpineAtlasLoader {
    type Asset = SpineSkeleton;
    type Settings = ();
    type Error = anyhow::Error;

    async fn load<'a>(
        &'a self,
        reader: &'a mut Reader<'_>,
        _settings: &'a (),
        load_context: &'a mut LoadContext<'_>,
    ) -> Result<SpineSkeleton> {
        let path = load_context.path().to_path_buf();

        let data = read_string(reader).await?;
        let atlas = parse_atlas(load_context, &path, &data)
            .with_context(|| format!("When loading atlas {:?}", path.display()))?;
        Ok(make_atlas_skeleton(&path.display().to_string(), atlas)?.into_asset())
    }

    fn extensions(&self) -> &[&str] {
        &["atlas"]
    }
}

async fn read_string(reader: &mut Reader<'_>) -> Result<String> {
    let mut content = String::new();
    reader.read_to_string(&mut content).await?;
    Ok(content)
}

/// Read a file next to the one being loaded. It becomes a dependency of the asset.
async fn read_sibling(load_context: &mut LoadContext<'_>, path: PathBuf) -> Result<String> {
    let bytes = load_context.read_asset_bytes(path).await?;
    Ok(String::from_utf8(bytes)?)
}

/// Parse the atlas file at `atlas_path` with the contents `data` and load its page texture.
/// The texture is a dependency of the asset being loaded.
fn parse_atlas(load_context: &mut LoadContext<'_>, atlas_path: &Path, data: &str) -> Result<Atlas> {
    let (mut atlas, page_name) = Atlas::from_string(data)?;
    let base_dir = atlas_path.parent().with_context(|| {
        format!(
            "Could not determine parent directory of {:?}",
            atlas_path.display()
        )
    })?;
    atlas.set_texture(load_atlas_texture(load_context, base_dir.join(page_name)));
    Ok(atlas)
}

/// Load the texture of an atlas page (linear sampling).
fn load_atlas_texture(load_context: &mut LoadContext<'_>, path: impl AsRef<Path>) -> Handle<Image> {
    load_context
        .loader()
        .with_settings(|s: &mut ImageLoaderSettings| {
            s.sampler = ImageSampler::linear();
        })
        .load(path.as_ref().to_owned())
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use bevy::asset::{AssetPlugin, LoadState};

    use super::*;

    /// An app that loads assets from the `assets` directory of this crate with the loaders of this
    /// module (but without image loaders, so the textures fail to load, which does not matter
    /// here)
    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin {
                file_path: format!("{}/assets", env!("CARGO_MANIFEST_DIR")),
                ..default()
            },
        ))
        .init_asset::<Image>()
        .init_asset::<SpineSkeleton>()
        .init_asset_loader::<SpineJsonLoader>()
        .init_asset_loader::<SpineAtlasLoader>();
        app
    }

    /// Load `path` and wait until it is loaded (or failed)
    fn load(app: &mut App, path: &str) -> Handle<SpineSkeleton> {
        let handle = app.world().resource::<AssetServer>().load(path.to_string());
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(10) {
            app.update();
            match app.world().resource::<AssetServer>().load_state(&handle) {
                LoadState::Loaded | LoadState::Failed(_) => break,
                _ => std::thread::sleep(Duration::from_millis(1)),
            }
        }
        handle
    }

    fn skeleton<'a>(app: &'a App, handle: &Handle<SpineSkeleton>) -> &'a SpineSkeleton {
        let state = app.world().resource::<AssetServer>().load_state(handle);
        app.world()
            .resource::<Assets<SpineSkeleton>>()
            .get(handle)
            .unwrap_or_else(|| panic!("not loaded: {state:?}"))
    }

    #[test]
    fn loads_json_through_the_asset_server() {
        let mut app = app();
        let handle = load(&mut app, "animations/ella/Ella.json");

        let skeleton = skeleton(&app, &handle);
        assert!(skeleton.info().has_animation("walk_l"));
        assert!(!skeleton.vertices.is_empty());
        // The page texture is a dependency, loaded next to the atlas
        let texture = skeleton.texture();
        let path = app.world().resource::<AssetServer>().get_path(texture.id());
        assert_eq!(path.unwrap().path(), Path::new("animations/ella/Ella.png"));
    }

    #[test]
    fn multi_skin_files_have_a_labeled_asset_per_skin() {
        let mut app = app();
        let path = "animations/example01_skins/Spine.json";
        let root = load(&mut app, path);
        let default = load(&mut app, &format!("{path}#skin/default"));
        let alt = load(&mut app, &format!("{path}#skin/alt"));
        let skeletons = app.world().resource::<Assets<SpineSkeleton>>();
        assert!(skeletons.get(&root).is_some());
        assert!(skeletons.get(&default).is_some());
        assert!(skeletons.get(&alt).is_some());
        assert_ne!(default.id(), alt.id());
    }

    #[test]
    fn loads_atlas_skeleton() {
        let mut app = app();
        let handle = load(&mut app, "animations/example04/skeleton.atlas");

        let skeleton = skeleton(&app, &handle);
        assert!(!skeleton.animations.is_empty());
        // The bounds come from the regions
        assert!(skeleton.skeleton.size.min_element() > 0.0);
    }

    #[test]
    fn failing_loads_are_reported_as_failed() {
        let mut app = app();
        let handle = load(&mut app, "animations/does/not/Exist.json");

        let state = app.world().resource::<AssetServer>().load_state(&handle);
        assert!(matches!(state, LoadState::Failed(_)), "{state:?}");
    }
}
