use bevy::{color::palettes::css, math::vec2, prelude::*, render::primitives::Aabb};

use crate::{InstanceInfos, SkeletonInstance};

#[derive(Resource, Default)]
pub struct SpineGPUDebugging {
    pub draw_bones: bool,
    pub draw_aabbs: bool,
}

/// Draw bones as little circles for debugging
pub(crate) fn debug_draw_bones(
    mut gizmos: Gizmos,
    instance_infos: Res<InstanceInfos>,
    debug: Option<Res<SpineGPUDebugging>>,
) {
    if !debug.map(|d| d.draw_bones).unwrap_or(false) {
        return;
    }

    let r = 10.0;
    for instances in instance_infos.types().values() {
        for bone in &instances.bones {
            gizmos.circle_2d(bone.translation, r, Color::WHITE);
            gizmos.line_2d(
                bone.translation,
                bone.translation + bone.matrix2 * vec2(r, 0.0),
                Color::WHITE,
            );
        }
    }
}

pub(crate) fn debug_aabb(
    query: Query<(&Aabb, &GlobalTransform), With<SkeletonInstance>>,
    mut gizmos: Gizmos,
    debug: Option<Res<SpineGPUDebugging>>,
) {
    if !debug.map(|d| d.draw_aabbs).unwrap_or(false) {
        return;
    }

    for (aabb, gt) in &query {
        let (_, _, tr) = gt.to_scale_rotation_translation();
        gizmos.circle_2d(tr.xy(), 8.0, css::DEEP_PINK);

        gizmos.rect_2d(
            aabb.center.xy(),
            0.0,
            aabb.half_extents.xy() * 2.0,
            css::DEEP_PINK,
        );
    }
}
