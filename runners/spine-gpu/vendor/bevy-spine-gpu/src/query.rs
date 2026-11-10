use bevy::{ecs::query::QueryData, prelude::*};

use crate::{
    AnimationControl, AnimationPlayback, LoadSpine, LoadSpineSettings, Loaded, PendingSpine,
    SkeletonInstance,
};

/// Convenience query param for ensuring a certain state of the skleton/animation.
#[derive(QueryData)]
#[query_data(mutable, derive(Debug))]
pub struct SpineGPUQuery {
    entity: Entity,
    control: Option<&'static mut AnimationControl>,
    playback: Option<&'static mut AnimationPlayback>,
    skeleton: Option<&'static mut SkeletonInstance>,
    loaded: Option<&'static Loaded>,
    pending: Option<&'static mut PendingSpine>,
}

impl<'w> SpineGPUQueryItem<'w> {
    /// Ensure the skeleton & animation are in the state described by the parameters.
    /// In particular:
    ///   `None`: Remove all SpineGPU components from the entity
    ///   `Some(loadspine, settings)`: If the entity does not have the skeleton
    ///     specified in `loadspine`, attach `loadspine` and `settings`
    ///     to trigger loading it (unless it is being loaded already, then
    ///     only the settings are updated).
    ///     Otherwise, update:
    ///     - The animation label (& reset timer only if it was actually changed)
    ///     - The skeleton color and base transform
    ///     from `loadspine` and `settings`.
    pub fn ensure_state(
        &mut self,
        desired: Option<(LoadSpine, LoadSpineSettings)>,
        commands: &mut Commands,
    ) {
        // Note: Under the hood bevy turns our mut references to components
        // into `Mut`s (just like with normal queries),
        // so change detection is only triggered when we mut deref a member.

        let Some((loadspine, settings)) = desired else {
            commands.entity(self.entity).remove::<(
                AnimationControl,
                AnimationPlayback,
                SkeletonInstance,
                LoadSpine,
                LoadSpineSettings,
                PendingSpine,
            )>();
            return;
        };

        if Some(&loadspine) != self.loaded.map(|l| &l.loadspine) {
            if let Some(pending) = self.pending.as_mut()
                && pending.loadspine == loadspine
            {
                // Already being loaded, the settings are applied once it is done
                pending.settings = Some(settings);
                return;
            }
            commands.entity(self.entity).insert((loadspine, settings));
            return;
        }

        if self.pending.is_some() {
            // Switched back to the loaded skeleton while another one was loading
            commands.entity(self.entity).remove::<PendingSpine>();
        }

        if let Some(control) = self.control.as_mut() {
            if settings.transition_duration != control.transition_duration() {
                control.set_transition_duration(settings.transition_duration);
            }

            if settings.animation != control.label() {
                control.set_label_with_transition(
                    settings.animation.clone(),
                    settings.transition_duration,
                );
                if let Some(playback) = self.playback.as_mut() {
                    playback.set_current_time_seconds(0.0);
                }
            }
        }

        if let Some(skeleton) = self.skeleton.as_mut() {
            if settings.color != skeleton.color() {
                skeleton.set_color(settings.color);
            }
            if settings.base_transform != skeleton.base_transform() {
                skeleton.set_base_transform(settings.base_transform);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce as _;

    use super::*;

    fn ensure_state(world: &mut World, entity: Entity, loadspine: &LoadSpine, animation: &str) {
        let loadspine = loadspine.clone();
        let settings = LoadSpineSettings {
            animation: animation.into(),
            ..default()
        };
        world.run_system_once(move |mut q: Query<SpineGPUQuery>, mut commands: Commands| {
            let mut item = q.get_mut(entity).unwrap();
            item.ensure_state(Some((loadspine.clone(), settings.clone())), &mut commands);
        });
        world.flush();
    }

    #[test]
    fn ensure_state_updates_the_settings_of_a_pending_request() {
        let mut world = World::new();
        let loadspine = LoadSpine::StaticImagePath("x.png".into(), UVec2::ONE);
        let entity = world.spawn(LoadSpine::spine_directory("unrelated")).id();
        // The previous request is replaced by the new one
        ensure_state(&mut world, entity, &loadspine, "idle");
        assert_eq!(world.get::<LoadSpine>(entity), Some(&loadspine));

        // `load_spine` turns the request into a pending one
        world
            .entity_mut(entity)
            .remove::<(LoadSpine, LoadSpineSettings)>()
            .insert(PendingSpine::for_tests(loadspine.clone()));
        ensure_state(&mut world, entity, &loadspine, "walk");

        // Not requested again, but the pending request got the new settings
        assert!(world.get::<LoadSpine>(entity).is_none());
        let pending = world.get::<PendingSpine>(entity).unwrap();
        assert_eq!(pending.settings.as_ref().unwrap().animation, "walk");

        // A different skeleton wins over the pending one
        let other = LoadSpine::spine_directory("other");
        ensure_state(&mut world, entity, &other, "walk");
        assert_eq!(world.get::<LoadSpine>(entity), Some(&other));
    }

    #[test]
    fn ensure_state_drops_a_pending_request_when_switching_back() {
        let mut world = World::new();
        let loaded = LoadSpine::spine_directory("a");
        let entity = world
            .spawn((
                SkeletonInstance::new(default()),
                AnimationControl::default(),
                AnimationPlayback::default(),
                Loaded {
                    loadspine: loaded.clone(),
                    settings: None,
                },
                PendingSpine::for_tests(LoadSpine::spine_directory("b")),
            ))
            .id();

        ensure_state(&mut world, entity, &loaded, "");

        assert!(world.get::<PendingSpine>(entity).is_none());
        assert!(world.get::<LoadSpine>(entity).is_none());
    }
}
