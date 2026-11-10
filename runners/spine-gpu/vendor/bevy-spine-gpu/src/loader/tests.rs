use super::build::animation_durations;
use super::spine_json::{Attachment, DEFAULT_SKIN_NAME, parse};
use super::*;
use crate::{Animation, Atlas, Timeline};
use std::collections::HashMap;

const REGION_WITH_SCALE: &str = r#"
    {
        "skeleton": { "hash": "", "spine": "4.2.43", "width": 10, "height": 10 },
        "bones": [{ "name": "root", "scaleX": 4, "scaleY": 4 }],
        "slots": [{ "name": "body", "bone": "root", "attachment": "body" }],
        "skins": [
            {
                "name": "default",
                "attachments": {
                    "body": {
                        "body": {
                            "scaleX": 0.25,
                            "scaleY": 0.25,
                            "width": 10.0,
                            "height": 10.0
                        }
                    }
                }
            }
        ],
        "animations": {}
    }
    "#;

#[test]
fn region_attachment_parses_scale() {
    let spine = parse(REGION_WITH_SCALE).unwrap();
    let skin = spine.select_skin(None, "test.json").unwrap();
    let attachment = skin.attachment("body", "body").unwrap();

    match attachment {
        Attachment::Rectangle {
            scale_x, scale_y, ..
        } => {
            assert_eq!(*scale_x, 0.25);
            assert_eq!(*scale_y, 0.25);
        }
        other => panic!("expected Attachment::Rectangle, got {other:?}"),
    }
}

const MULTI_SKIN_LINKED_MESH: &str = r#"
    {
        "skeleton": { "hash": "", "spine": "4.2.43", "width": 10, "height": 10 },
        "bones": [{ "name": "root" }],
        "slots": [{ "name": "leg", "bone": "root", "attachment": "leg" }],
        "skins": [
            {
                "name": "Red",
                "attachments": {
                    "leg": {
                        "leg": {
                            "type": "mesh",
                            "path": "Red/leg",
                            "uvs": [0, 0, 1, 0, 1, 1],
                            "triangles": [0, 1, 2],
                            "vertices": [0, 0, 1, 0, 1, 1],
                            "width": 10.0,
                            "height": 10.0
                        }
                    }
                }
            },
            {
                "name": "Blue",
                "attachments": {
                    "leg": {
                        "leg": {
                            "type": "linkedmesh",
                            "path": "Blue/leg",
                            "skin": "Red",
                            "parent": "leg",
                            "width": 10.0,
                            "height": 10.0
                        }
                    }
                }
            }
        ],
        "animations": {}
    }
    "#;

const DEFAULT_SKIN_FALLBACK: &str = r#"
    {
        "skeleton": { "hash": "", "spine": "4.2.43", "width": 10, "height": 10 },
        "bones": [{ "name": "root" }],
        "slots": [
            { "name": "Pickaxe", "bone": "root" },
            { "name": "Head", "bone": "root", "attachment": "Head" }
        ],
        "skins": [
            {
                "name": "default",
                "attachments": {
                    "Pickaxe": {
                        "Hammer": {
                            "path": "Generic/Hammer",
                            "width": 10.0,
                            "height": 10.0
                        }
                    },
                    "Head": {
                        "Head": {
                            "path": "Generic/Head",
                            "width": 10.0,
                            "height": 10.0
                        }
                    }
                }
            },
            {
                "name": "Blue",
                "attachments": {
                    "Head": {
                        "Head": {
                            "path": "Blue/Head",
                            "width": 10.0,
                            "height": 10.0
                        }
                    }
                }
            }
        ],
        "animations": {
            "dig": {
                "slots": {
                    "Pickaxe": {
                        "attachment": [{ "name": "Hammer" }]
                    }
                }
            }
        }
    }
    "#;

#[test]
fn selected_skin_linked_mesh_uses_selected_region_and_parent_mesh() {
    let spine = parse(MULTI_SKIN_LINKED_MESH).unwrap();
    let skin = spine.select_skin(Some("Blue"), "test.json").unwrap();
    let attachment = skin.attachment("leg", "leg").unwrap();

    let (resolved, region_name) = attachment
        .resolve(&spine, "leg", "leg", &mut Vec::new())
        .unwrap();

    assert_eq!(region_name, "Blue/leg");
    assert!(matches!(resolved, Attachment::Mesh { .. }));
}

#[test]
fn selected_skin_attachment_lookup_falls_back_to_default_skin() {
    let spine = parse(DEFAULT_SKIN_FALLBACK).unwrap();
    let skin = spine.select_skin(Some("Blue"), "test.json").unwrap();
    let default_skin = spine.find_skin(DEFAULT_SKIN_NAME);

    let (attachment_skin, attachment) = skin
        .attachment_with_fallback(default_skin, "Pickaxe", "Hammer")
        .unwrap();

    assert_eq!(attachment_skin.name, DEFAULT_SKIN_NAME);
    assert_eq!(attachment.region_name("Hammer"), "Generic/Hammer");
}

#[test]
fn effective_skin_attachments_include_default_entries_and_selected_overrides() {
    let spine = parse(DEFAULT_SKIN_FALLBACK).unwrap();
    let skin = spine.select_skin(Some("Blue"), "test.json").unwrap();
    let default_skin = spine.find_skin(DEFAULT_SKIN_NAME);
    let attachments = skin.attachments_with_fallback(default_skin);

    let hammer = attachments
        .iter()
        .find(|(slot_name, attachment_name, _, _)| {
            slot_name == "Pickaxe" && attachment_name == "Hammer"
        })
        .unwrap();
    assert_eq!(hammer.2.name, DEFAULT_SKIN_NAME);

    let head = attachments
        .iter()
        .find(|(slot_name, attachment_name, _, _)| slot_name == "Head" && attachment_name == "Head")
        .unwrap();
    assert_eq!(head.2.name, "Blue");
    assert_eq!(head.3.region_name("Head"), "Blue/Head");
}

#[test]
fn copies_animation_durations_for_state_lookup() {
    let animations = HashMap::from([(
        "climb_up_r".to_string(),
        Animation {
            duration: 1.25,
            bone_timelines: default(),
            slot_timelines: default(),
            events_timeline: Timeline::empty(),
        },
    )]);

    assert_eq!(
        animation_durations(&animations).get("climb_up_r"),
        Some(&1.25)
    );
}

#[test]
fn parses_real_asset() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/assets/animations/ella/Ella.json"
    );
    let json = std::fs::read_to_string(path).unwrap();
    let spine = parse(&json).unwrap();

    assert_eq!(spine.bones.len(), 27);
    assert_eq!(spine.animations.len(), 14);
    assert!(spine.animations.contains_key("walk_l"));
}

/// Parse the files of a real skeleton the way the loader does
fn real_skeleton_file(base: &str) -> (build::SpineFile, Atlas) {
    let json = std::fs::read_to_string(format!("{base}.json")).unwrap();
    let atlas_data = std::fs::read_to_string(format!("{base}.atlas")).unwrap();
    let (mut atlas, _page_name) = Atlas::from_string(&atlas_data).unwrap();
    atlas.set_texture(Handle::default());
    let file = build::parse_spine_file(&format!("{base}.json"), &json).unwrap();
    (file, atlas)
}

#[test]
fn builds_real_skeleton_from_strings() {
    let base = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/animations/ella/Ella");
    let (file, atlas) = real_skeleton_file(base);

    let skeleton = file.build(None, atlas).unwrap().into_asset();
    assert!(!skeleton.animations.is_empty());
    assert!(!skeleton.vertices.is_empty());
    assert!(skeleton.info().has_animation("idle_l"));
}

#[test]
fn builds_every_skin_of_a_multi_skin_file() {
    let base = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/assets/animations/example01_skins/Spine"
    );
    let (file, atlas) = real_skeleton_file(base);

    let skins: Vec<_> = file.skin_names().collect();
    for name in ["default", "alt"] {
        assert!(skins.contains(&name), "{name} not in {skins:?}");
    }
    for skin in skins {
        let skeleton = file.build(Some(skin), atlas.clone()).unwrap().into_asset();
        assert!(!skeleton.vertices.is_empty(), "{skin}");
    }
    // The root asset is the default skin
    assert!(file.build(None, atlas).is_ok());
}

#[test]
fn parse_spine_file_reports_invalid_json() {
    let err = build::parse_spine_file("dir/Broken.json", "not json")
        .err()
        .unwrap();
    assert!(format!("{err:?}").contains("dir/Broken.json"));
}

const COLORED: &str = r#"
    {
        "skeleton": { "hash": "", "spine": "4.2.43", "width": 10, "height": 10 },
        "bones": [{ "name": "root" }],
        "slots": [
            { "name": "tinted", "bone": "root", "attachment": "body", "color": "ff8000ff", "dark": "000000" },
            { "name": "plain", "bone": "root", "attachment": "body" },
            { "name": "attached", "bone": "root", "attachment": "body" }
        ],
        "skins": [
            {
                "name": "default",
                "attachments": {
                    "tinted": { "body": { "color": "80808080", "width": 10.0, "height": 10.0 } },
                    "plain": { "body": { "width": 10.0, "height": 10.0 } },
                    "attached": { "body": { "color": "c363e4ff", "width": 10.0, "height": 10.0 } }
                }
            }
        ],
        "animations": {
            "fade": { "slots": { "plain": { "rgba": [{ "time": 0, "color": "ffffffff" }] } } }
        }
    }
    "#;

#[test]
fn slot_color_times_attachment_color_reaches_vertices() {
    let (mut atlas, _) = Atlas::from_string(
        "test.ktx2
size:16,16
filter:Linear,Linear
pma:true
body
bounds:0,0,10,10
",
    )
    .unwrap();
    atlas.set_texture(Handle::default());

    let file = build::parse_spine_file("test.json", COLORED).unwrap();
    let skeleton = file.build(None, atlas).unwrap().into_asset();

    // One rectangle (4 vertices) per slot, in no particular order
    assert_eq!(skeleton.vertices.len(), 12);
    let mut colors: Vec<u32> = skeleton
        .vertices
        .chunks(4)
        .map(|quad| {
            assert!(quad.iter().all(|v| v.color == quad[0].color));
            quad[0].color
        })
        .collect();
    colors.sort();
    let mut expected = [
        // "tinted": ff8000ff x 80808080 = 80, 40, 00, 80 (r in the low byte)
        0x8000_4080,
        // "plain": no slot color, no attachment color: white
        0xffff_ffff,
        // "attached": white slot, attachment color c363e4ff only
        0xffe4_63c3,
    ];
    expected.sort();
    assert_eq!(colors, expected);
}

#[test]
fn linked_mesh_uses_its_own_color() {
    let json = MULTI_SKIN_LINKED_MESH
        .replace(
            r#""path": "Blue/leg","#,
            r#""path": "Blue/leg", "color": "11223344","#,
        )
        .replace(
            r#""path": "Red/leg","#,
            r#""path": "Red/leg", "color": "ffffffff","#,
        );
    let spine = parse(&json).unwrap();
    let skin = spine.select_skin(Some("Blue"), "test.json").unwrap();
    let (resolved, _) = skin
        .attachment("leg", "leg")
        .unwrap()
        .resolve(&spine, "leg", "leg", &mut Vec::new())
        .unwrap();
    assert_eq!(resolved.color().0, [0x11, 0x22, 0x33, 0x44]);
}

#[test]
fn slot_timeline_with_only_color_parses() {
    let spine = parse(COLORED).unwrap();
    let timelines = &spine.animations["fade"].slots["plain"];
    assert!(timelines.attachment.is_empty());
    assert!(timelines.has_color_timelines());
    assert!(spine.has_color_timelines());

    let spine = parse(REGION_WITH_SCALE).unwrap();
    assert!(!spine.has_color_timelines());
}
