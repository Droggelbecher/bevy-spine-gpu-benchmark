use std::collections::HashMap;

use bevy::{asset::LoadState, prelude::*};

use crate::{
    AnimationControl, AnimationPlayback, SkeletonDrawGroup, SkeletonDrawGroups, SkeletonInstance,
    SpineSkeleton,
};

use super::{LoadSpine, LoadSpineSettings, Loaded, asset::SKIN_LABEL_PREFIX, build};

/// `LoadSpine` -> skeleton type. The cache keeps the handles, so types stay loaded once requested.
#[derive(Resource, Default)]
pub(crate) struct LoadSpineCache {
    cache: HashMap<LoadSpine, Handle<SpineSkeleton>>,
}

/// A request (`LoadSpine`) that is being loaded: the entity gets its `SkeletonInstance` and other
/// components once the skeleton and its texture are available, see `finish_pending_spine`.
#[derive(Component, Debug)]
pub(crate) struct PendingSpine {
    skeleton: Handle<SpineSkeleton>,
    /// For a skin of a file: the file's main skeleton. A skin that does not exist never finishes
    /// loading, so this tells that the file has been loaded without the skin.
    file: Option<Handle<SpineSkeleton>>,
    pub(crate) loadspine: LoadSpine,
    pub(crate) settings: Option<LoadSpineSettings>,
}

#[cfg(test)]
impl PendingSpine {
    pub(crate) fn for_tests(loadspine: LoadSpine) -> Self {
        Self {
            skeleton: default(),
            file: None,
            loadspine,
            settings: Some(default()),
        }
    }
}

/// Path (and label) of the skeleton asset of a file based `LoadSpine`
fn asset_path(loadspine: &LoadSpine) -> Option<String> {
    match loadspine {
        LoadSpine::SpineDirectory(path, None) => Some(format!("{path}.json")),
        LoadSpine::SpineDirectory(path, Some(skin)) => {
            Some(format!("{path}.json#{SKIN_LABEL_PREFIX}{skin}"))
        }
        LoadSpine::Atlas(path) => Some(path.clone()),
        LoadSpine::StaticImage(..) | LoadSpine::StaticImagePath(..) => None,
    }
}

/// Start loading the skeleton type of `loadspine`, or create it right away for in-memory ones.
fn request_skeleton(
    loadspine: &LoadSpine,
    asset_server: &AssetServer,
    skeletons: &mut Assets<SpineSkeleton>,
) -> anyhow::Result<Handle<SpineSkeleton>> {
    if let Some(path) = asset_path(loadspine) {
        return Ok(asset_server.load(path));
    }
    let data = match loadspine {
        LoadSpine::StaticImagePath(path, size) => {
            build::make_image_skeleton(asset_server.load(path), *size)
        }
        LoadSpine::StaticImage(texture, size) => build::make_image_skeleton(texture.clone(), *size),
        LoadSpine::SpineDirectory(..) | LoadSpine::Atlas(..) => unreachable!(),
    }?;
    Ok(skeletons.add(data.into_asset()))
}

/// Resolve the `LoadSpine` requests to skeleton types and start loading them.
pub(crate) fn load_spine(
    query: Query<(Entity, &LoadSpine, Option<&LoadSpineSettings>)>,
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut skeletons: ResMut<Assets<SpineSkeleton>>,
    mut draw_groups: ResMut<SkeletonDrawGroups>,
    mut cache: ResMut<LoadSpineCache>,
) {
    for (entity, loadspine, settings) in &query {
        let skeleton = if let Some(skeleton) = cache.cache.get(loadspine) {
            skeleton.clone()
        } else {
            match request_skeleton(loadspine, &asset_server, &mut skeletons) {
                Ok(skeleton) => {
                    cache.cache.insert(loadspine.clone(), skeleton.clone());
                    skeleton
                }
                Err(e) => {
                    error!("Failed to load {loadspine:?}: {e:?}");
                    commands
                        .entity(entity)
                        .remove::<(LoadSpine, LoadSpineSettings)>();
                    continue;
                }
            }
        };

        let requested_group = SkeletonDrawGroup::from_settings(settings);
        let type_group = match draw_groups.group(skeleton.id()) {
            Some(group) => group,
            None => draw_groups.register(skeleton.id(), requested_group),
        };
        if settings.is_some() && type_group != requested_group {
            warn_once!(
                "{loadspine:?} requested with draw group {requested_group:?}, but its \
                 skeleton type already uses {type_group:?} (the first request wins)"
            );
        }

        let file = match loadspine {
            LoadSpine::SpineDirectory(path, Some(_)) => {
                Some(asset_server.load(format!("{path}.json")))
            }
            _ => None,
        };

        // Replaces a request that is still pending: the latest request wins
        commands
            .entity(entity)
            .insert(PendingSpine {
                skeleton,
                file,
                loadspine: loadspine.clone(),
                settings: settings.cloned(),
            })
            .remove::<(LoadSpine, LoadSpineSettings)>();
    }
}

/// Turn the pending requests, whose skeleton and texture are loaded, into skeleton instances.
///
/// Only then do the entities get a `SkeletonInstance` and `Loaded`, so these mean that the
/// skeleton is ready to be drawn and its `SkeletonInfo` available.
/// Requests that fail to load are logged and dropped.
pub(crate) fn finish_pending_spine(
    query: Query<(Entity, &PendingSpine, Has<GlobalTransform>)>,
    skeletons: Res<Assets<SpineSkeleton>>,
    images: Res<Assets<Image>>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
) {
    for (entity, pending, has_spatial) in &query {
        let PendingSpine {
            skeleton: handle,
            file,
            loadspine,
            settings,
        } = pending;

        let Some(skeleton) = skeletons.get(handle) else {
            if let LoadState::Failed(e) = asset_server.load_state(handle.id()) {
                error!("Failed to load {loadspine:?}: {e}");
                commands.entity(entity).remove::<PendingSpine>();
            } else if let Some(file) = file {
                match asset_server.load_state(file.id()) {
                    LoadState::Failed(e) => {
                        error!("Failed to load {loadspine:?}: {e}");
                        commands.entity(entity).remove::<PendingSpine>();
                    }
                    LoadState::Loaded => {
                        error!("Failed to load {loadspine:?}: the file has no such skin");
                        commands.entity(entity).remove::<PendingSpine>();
                    }
                    _ => {}
                }
            }
            continue;
        };
        // The skeleton is available before its texture
        let texture = skeleton.texture();
        if !images.contains(texture.id()) {
            if let Some(LoadState::Failed(e)) = asset_server.get_load_state(texture.id()) {
                error!("Failed to load the texture of {loadspine:?}: {e}");
                commands.entity(entity).remove::<PendingSpine>();
            }
            continue;
        }

        let mut skel_ref = SkeletonInstance::new(handle.clone());
        let mut control = AnimationControl::default();
        let mut playback = AnimationPlayback::default();

        if let Some(settings) = settings {
            control.label = settings.animation.clone();
            control.time_scale = settings.time_scale;
            control.transition_duration = settings.transition_duration;
            // Same label in playback: the animation starts without a transition
            playback.label = settings.animation.clone();
            skel_ref.color = settings.color;
            skel_ref.base_transform = settings.base_transform;
        }

        commands.entity(entity).insert((
            skel_ref,
            control,
            playback,
            Loaded {
                loadspine: loadspine.clone(),
                settings: settings.clone(),
            },
        ));

        if !has_spatial {
            warn!("Entity {entity:?} has LoadSpine but no global transform, inserting default.");
            commands.entity(entity).insert(SpatialBundle::default());
        }

        commands.entity(entity).remove::<PendingSpine>();
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use bevy::asset::AssetPlugin;

    use super::*;

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
        .init_asset_loader::<crate::SpineJsonLoader>()
        .init_resource::<LoadSpineCache>()
        .init_resource::<SkeletonDrawGroups>()
        .add_systems(Update, (load_spine, finish_pending_spine).chain());
        app
    }

    /// Update until `done` or give up
    fn update_until(app: &mut App, done: impl Fn(&World) -> bool) {
        let start = Instant::now();
        while !done(app.world()) && start.elapsed() < Duration::from_secs(10) {
            app.update();
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn static_image_becomes_an_instance_once_its_texture_is_loaded() {
        let mut app = app();
        let texture = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .reserve_handle();
        let settings = LoadSpineSettings {
            animation: "image".into(),
            sorted: true,
            ..default()
        };
        let entity = app
            .world_mut()
            .spawn((
                LoadSpine::StaticImage(texture.clone(), UVec2::new(10, 20)),
                settings,
            ))
            .id();

        app.update();
        // Waiting for the texture
        let world = app.world();
        assert!(world.get::<PendingSpine>(entity).is_some());
        assert!(world.get::<LoadSpine>(entity).is_none());
        assert!(world.get::<SkeletonInstance>(entity).is_none());

        app.world_mut()
            .resource_mut::<Assets<Image>>()
            .insert(texture.id(), Image::default());
        app.update();

        let world = app.world();
        assert!(world.get::<PendingSpine>(entity).is_none());
        assert!(world.get::<Loaded>(entity).is_some());
        assert_eq!(
            world.get::<AnimationControl>(entity).unwrap().label(),
            "image"
        );
        let instance = world.get::<SkeletonInstance>(entity).unwrap();
        let info = instance.info(world.resource()).unwrap();
        assert!(info.slot_index("image").is_some());
        assert!(
            world
                .resource::<SkeletonDrawGroups>()
                .is_sorted(instance.skeleton().id())
        );
    }

    #[test]
    fn requests_of_the_same_skeleton_share_the_type_and_its_first_draw_group() {
        let mut app = app();
        let texture = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .add(Image::default());
        let request = LoadSpine::StaticImage(texture, UVec2::ONE);
        let first = app
            .world_mut()
            .spawn((
                request.clone(),
                LoadSpineSettings {
                    render_bucket: 3,
                    ..default()
                },
            ))
            .id();
        let second = app
            .world_mut()
            .spawn((
                request,
                LoadSpineSettings {
                    sorted: true,
                    ..default()
                },
            ))
            .id();

        app.update();

        let world = app.world();
        let id = |e| world.get::<SkeletonInstance>(e).unwrap().skeleton().id();
        assert_eq!(id(first), id(second));
        assert_eq!(world.resource::<Assets<SpineSkeleton>>().len(), 1);
        assert!(!world.resource::<SkeletonDrawGroups>().is_sorted(id(first)));
    }

    #[test]
    fn failing_requests_are_dropped_without_panicking() {
        let mut app = app();
        let missing = app
            .world_mut()
            .spawn(LoadSpine::spine_directory("animations/does/not/Exist"))
            .id();
        let no_skin = app
            .world_mut()
            .spawn(LoadSpine::spine_directory_with_skin(
                "animations/example01/Spine",
                "Nobody",
            ))
            .id();

        // Pending first
        app.update();
        assert!(app.world().get::<PendingSpine>(missing).is_some());
        update_until(&mut app, |world| {
            world.get::<PendingSpine>(missing).is_none()
                && world.get::<PendingSpine>(no_skin).is_none()
        });

        let world = app.world();
        for entity in [missing, no_skin] {
            assert!(world.get::<PendingSpine>(entity).is_none());
            assert!(world.get::<LoadSpine>(entity).is_none());
            assert!(world.get::<SkeletonInstance>(entity).is_none());
        }
    }
}
