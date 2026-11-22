//! Acceptance tests of lot 5.3: a full map built through the editor API,
//! symmetry twins, prefab instances, library thumbnails and the new tools
//! driven by pointer events.

use std::path::PathBuf;

use faset_core::glam::Vec3;
use faset_editor::{Editor, Library, PlaceSettings, Ray, Selection, Tool, THUMBNAIL_SIZE};
use faset_mapfmt::geometry::build_meshes;
use faset_mapfmt::{
    expand, expand_prefabs, parse, Brush, Dir, Light, MapFile, Physics, Pickup, PickupKind, Prop,
    Spawn, Symmetry, Team, WeatherSlot, Zone, ZoneKind,
};

fn boxed(min: [i32; 3], max: [i32; 3]) -> Brush {
    Brush::Box {
        min,
        max,
        material: "concrete".into(),
    }
}

fn props_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/props")
}

fn down(x: f32, z: f32) -> Ray {
    Ray::new(Vec3::new(x, 60.0, z), Vec3::NEG_Y)
}

fn library() -> Library {
    let (lib, errors) = Library::load_dir(&props_dir());
    assert!(errors.is_empty(), "{errors:?}");
    lib
}

/// A complete map, made only with editor calls.
fn build_full_map(editor: &mut Editor) {
    let mut meta = editor.map.meta.clone();
    meta.name = "Yard".into();
    meta.author = "Test".into();
    meta.description = "Two halves".into();
    meta.players = (4, 8);
    editor.set_meta(meta);

    let mut theme = editor.map.theme.clone();
    theme.sky = "clear".into();
    theme.weather = vec![
        WeatherSlot {
            from_s: 0.0,
            weather: "clear".into(),
        },
        WeatherSlot {
            from_s: 120.0,
            weather: "rain".into(),
        },
    ];
    editor.set_theme(theme);
    editor.set_physics(Some(Physics {
        gravity: Some(8.0),
        speed_scale: None,
    }));

    editor.add_brush(boxed([-40, -2, -40], [40, 0, 40])); // floor
    editor.add_brush(boxed([-40, 0, -40], [40, 8, -38])); // north wall
    editor.add_brush(Brush::Stairs {
        min: [10, 0, 0],
        max: [16, 6, 6],
        rise: Dir::PosZ,
        steps: 6,
        material: "metal".into(),
    });

    editor.define_prefab(
        "hut",
        vec![boxed([0, 0, 0], [8, 6, 8]), boxed([0, 6, 0], [8, 7, 8])],
        vec![Prop {
            recipe: "crate_wood".into(),
            at: [2.0, 0.0, 2.0],
            yaw: 0.0,
            scale: 1.0,
        }],
    );
    editor.place_prefab("hut", [-30, 0, -20], 0);
    editor.place_prefab("hut", [20, 0, 10], 1);

    for (team, x) in [
        (Team::A, -10.0),
        (Team::A, -8.0),
        (Team::B, 8.0),
        (Team::B, 10.0),
    ] {
        editor.add_spawn(Spawn {
            at: [x, 0.0, 5.0],
            yaw: 0.0,
            team,
        });
    }
    editor.add_pickup(Pickup {
        kind: PickupKind::Health,
        at: [0.0, 0.0, 0.0],
        respawn_s: 20,
    });
    editor.add_pickup(Pickup {
        kind: PickupKind::Weapon("rifle".into()),
        at: [4.0, 0.0, -6.0],
        respawn_s: 30,
    });
    editor.add_zone(Zone::Box {
        min: [-200, -40, -200],
        max: [200, -20, 200],
        kind: ZoneKind::KillVolume,
    });
    editor.add_light(Light {
        at: [0.0, 6.0, 0.0],
        color: [1.0, 0.8, 0.6],
        intensity: 1.0,
        range: 12.0,
    });
    editor.add_prop(Prop {
        recipe: "barrel_metal".into(),
        at: [1.0, 0.0, 2.5],
        yaw: 30.0,
        scale: 1.0,
    });
}

#[test]
fn a_full_map_is_built_through_the_editor_saved_and_reloaded_identical() {
    let mut editor = Editor::blank("tmp");
    editor.set_library(library());
    build_full_map(&mut editor);

    let m = &editor.map;
    assert_eq!(m.brushes.len(), 3);
    assert_eq!(m.prefabs.len(), 1);
    assert_eq!(m.prefabs[0].place.len(), 2);
    assert_eq!(m.spawns.iter().filter(|s| s.team == Team::A).count(), 2);
    assert_eq!(m.spawns.iter().filter(|s| s.team == Team::B).count(), 2);
    assert_eq!(m.pickups.len(), 2);
    assert_eq!(m.zones.len(), 1);
    assert_eq!(m.theme.sky, "clear");
    assert_eq!(m.symmetry, Symmetry::None);

    let text = editor.to_ron();
    let reloaded: MapFile = parse(text.as_bytes()).unwrap();
    assert_eq!(reloaded, editor.map);
    assert_eq!(faset_mapfmt::to_ron(&reloaded), text);

    // The saved map builds and passes the structural rules (1 and 2).
    assert!(!build_meshes(&expand(&reloaded)).is_empty());
    let report = map_tool::validate_bytes(text.as_bytes());
    assert!(
        !report.has_rule(1) && !report.has_rule(2),
        "{}",
        report.to_json()
    );
}

#[test]
fn every_step_of_the_full_build_undoes_to_the_previous_state() {
    let mut editor = Editor::blank("tmp");
    editor.set_library(library());
    let start = editor.to_ron();
    build_full_map(&mut editor);
    let steps = editor.history.undo_len();
    assert!(steps >= 15);
    while editor.undo() {}
    assert_eq!(editor.to_ron(), start);
    while editor.redo() {}
    assert_eq!(editor.history.undo_len(), steps);
    assert_eq!(parse(editor.to_ron().as_bytes()).unwrap(), editor.map);
}

#[test]
fn mirror_x_adds_a_twin_and_undo_removes_both_in_one_step() {
    let mut editor = Editor::blank("m");
    editor.symmetry = Symmetry::MirrorX;
    editor.add_brush(boxed([4, 0, 2], [10, 3, 6]));
    assert_eq!(editor.map.brushes.len(), 2);
    assert_eq!(editor.history.undo_len(), 1);
    assert_eq!(editor.map.brushes[0], boxed([4, 0, 2], [10, 3, 6]));
    assert_eq!(editor.map.brushes[1], boxed([-10, 0, 2], [-4, 3, 6]));
    assert!(editor.undo());
    assert!(editor.map.brushes.is_empty());
    assert!(editor.redo());
    assert_eq!(editor.map.brushes.len(), 2);
}

#[test]
fn mirror_z_and_rotate_180_mirror_the_right_axes() {
    let mut z = Editor::blank("z");
    z.symmetry = Symmetry::MirrorZ;
    z.add_brush(boxed([4, 0, 2], [10, 3, 6]));
    assert_eq!(z.map.brushes[1], boxed([4, 0, -6], [10, 3, -2]));

    let mut r = Editor::blank("r");
    r.symmetry = Symmetry::Rotate180;
    r.add_brush(boxed([4, 0, 2], [10, 3, 6]));
    assert_eq!(r.map.brushes[1], boxed([-10, 0, -6], [-4, 3, -2]));
    r.add_prop(Prop {
        recipe: "crate_wood".into(),
        at: [3.0, 0.0, 2.0],
        yaw: 0.0,
        scale: 1.0,
    });
    assert_eq!(r.map.props[1].at, [-3.0, 0.0, -2.0]);
    assert_eq!(r.history.undo_len(), 2);
    assert!(r.undo() && r.undo());
    assert!(r.map.brushes.is_empty() && r.map.props.is_empty());
}

#[test]
fn spawns_pickups_zones_and_lights_get_twins_and_the_twin_spawn_flips_team() {
    let mut e = Editor::blank("s");
    e.symmetry = Symmetry::Rotate180;
    e.add_spawn(Spawn {
        at: [6.0, 0.0, 2.0],
        yaw: 0.0,
        team: Team::A,
    });
    e.add_pickup(Pickup {
        kind: PickupKind::Ammo,
        at: [6.0, 0.0, 2.0],
        respawn_s: 10,
    });
    e.add_zone(Zone::Box {
        min: [2, 0, 2],
        max: [6, 4, 6],
        kind: ZoneKind::Water,
    });
    e.add_light(Light {
        at: [6.0, 3.0, 2.0],
        color: [1.0; 3],
        intensity: 1.0,
        range: 5.0,
    });
    assert_eq!(e.map.spawns.len(), 2);
    assert_eq!(e.map.spawns[1].team, Team::B);
    assert_eq!(e.map.spawns[1].at, [-6.0, 0.0, -2.0]);
    assert_eq!(e.map.pickups.len(), 2);
    assert_eq!(e.map.zones.len(), 2);
    assert_eq!(e.map.lights.len(), 2);
    assert_eq!(e.history.undo_len(), 4);
    // Removing one removes its twin as well.
    e.remove_spawn(0);
    assert!(e.map.spawns.is_empty());
    assert_eq!(e.history.undo_len(), 5);
    assert!(e.undo());
    assert_eq!(e.map.spawns.len(), 2);
}

#[test]
fn deleting_the_selection_deletes_the_twin_too() {
    let mut e = Editor::blank("d");
    e.symmetry = Symmetry::MirrorX;
    e.add_brush(boxed([4, 0, 2], [10, 3, 6]));
    // Select the original and delete it: both go.
    e.selection = Some(Selection::Brush(0));
    e.delete_selection();
    assert!(e.map.brushes.is_empty());
    assert!(e.undo());
    assert_eq!(e.map.brushes.len(), 2);
    assert_eq!(e.map.brushes[1], boxed([-10, 0, 2], [-4, 3, 6]));
}

#[test]
fn prefab_instances_reference_one_definition() {
    let mut e = Editor::blank("p");
    e.define_prefab("hut", vec![boxed([0, 0, 0], [8, 6, 8])], Vec::new());
    e.place_prefab("hut", [0, 0, 0], 0);
    e.place_prefab("hut", [20, 0, 0], 0);
    e.place_prefab("hut", [40, 0, 0], 2);
    assert_eq!(e.map.prefabs.len(), 1);
    assert_eq!(e.map.prefabs[0].brushes.len(), 1);
    assert_eq!(e.map.prefabs[0].place.len(), 3);
    assert!(e.map.brushes.is_empty());
    let expanded = expand_prefabs(&e.map);
    assert_eq!(expanded.brushes.len(), 3);
    assert_eq!(expanded.brushes[1], boxed([20, 0, 0], [28, 6, 8]));
    // The mesh cache sees the instances.
    e.rebuild_meshes();
    assert!(e.meshes.chunks().count() > 0);
    // One undo removes one instance.
    assert!(e.undo());
    assert_eq!(e.map.prefabs[0].place.len(), 2);
}

#[test]
fn prefab_instances_under_symmetry_are_mirrored_instances() {
    for sym in [Symmetry::MirrorX, Symmetry::MirrorZ, Symmetry::Rotate180] {
        let mut e = Editor::blank("p");
        e.symmetry = sym;
        e.define_prefab(
            "wedge",
            vec![Brush::Wedge {
                min: [0, 0, 0],
                max: [6, 3, 4],
                rise: Dir::PosX,
                material: "metal".into(),
            }],
            vec![Prop {
                recipe: "crate_wood".into(),
                at: [1.0, 0.0, 1.5],
                yaw: 20.0,
                scale: 1.0,
            }],
        );
        e.place_prefab("wedge", [10, 0, 6], 1);
        let before = e.history.undo_len();
        // The mirrored result equals mirroring the expanded original.
        let mut reference = Editor::blank("r");
        reference.symmetry = sym;
        reference.define_prefab(
            "wedge",
            e.map.prefabs[0].brushes.clone(),
            e.map.prefabs[0].props.clone(),
        );
        reference.map.prefabs[0].place = vec![e.map.prefabs[0].place[0]];
        reference.map.symmetry = sym;
        let want = expand(&reference.map);
        let got = expand(&e.map);
        let mut want_b = want.brushes.clone();
        let mut got_b = got.brushes.clone();
        want_b.sort_by_key(|b| format!("{b:?}"));
        got_b.sort_by_key(|b| format!("{b:?}"));
        assert_eq!(got_b, want_b, "{sym:?}");
        assert_eq!(got.props.len(), want.props.len());
        for p in &want.props {
            assert!(
                got.props.iter().any(|q| {
                    q.recipe == p.recipe
                        && (Vec3::from_array(q.at) - Vec3::from_array(p.at)).length() < 1e-4
                        && ((q.yaw - p.yaw).rem_euclid(360.0) < 1e-3
                            || (q.yaw - p.yaw).rem_euclid(360.0) > 359.999)
                }),
                "{sym:?} {p:?}"
            );
        }
        // Defining and placing is two steps; the twin did not add any.
        assert_eq!(before, 2);
        assert!(e.undo());
        assert!(e.map.prefabs.iter().all(|p| p.place.is_empty()));
    }
}

#[test]
fn stairs_have_visible_steps() {
    let mut e = Editor::blank("st");
    e.add_brush(Brush::Stairs {
        min: [0, 0, 0],
        max: [8, 6, 8],
        rise: Dir::PosZ,
        steps: 6,
        material: "metal".into(),
    });
    let stairs: usize = build_meshes(&e.map).iter().map(|c| c.indices.len()).sum();
    let mut w = Editor::blank("w");
    w.add_brush(Brush::Wedge {
        min: [0, 0, 0],
        max: [8, 6, 8],
        rise: Dir::PosZ,
        material: "metal".into(),
    });
    let wedge: usize = build_meshes(&w.map).iter().map(|c| c.indices.len()).sum();
    assert!(stairs > wedge, "steps must add faces: {stairs} vs {wedge}");
}

#[test]
fn the_library_renders_a_thumbnail_for_every_prop() {
    let lib = library();
    assert!(lib.len() >= 100, "{}", lib.len());
    for p in lib.props() {
        assert_eq!(p.thumbnail.size, THUMBNAIL_SIZE);
        assert!(p.thumbnail.covered_pixels() > 50, "{}", p.id);
        assert!(p.bounds.max.y > p.bounds.min.y, "{}", p.id);
    }
    assert!(lib.prop("crate_wood").is_some());
    assert!(lib.search("cover").count() > 0);
}

#[test]
fn placement_tools_work_through_pointer_events() {
    let mut e = Editor::blank("tools");
    e.set_library(library());
    e.symmetry = Symmetry::MirrorX;
    e.place = PlaceSettings {
        recipe: "crate_wood".into(),
        ..PlaceSettings::default()
    };

    e.tool = Tool::Prop;
    e.pointer_press(&down(6.0, 3.0));
    assert_eq!(e.map.props.len(), 2);
    assert_eq!(e.map.props[0].at[0], 6.0);
    assert_eq!(e.map.props[1].at[0], -6.0);

    e.tool = Tool::Spawn;
    e.pointer_press(&down(6.0, 3.0));
    assert_eq!(e.map.spawns.len(), 2);
    assert_eq!(e.map.spawns[0].team, Team::A);
    assert_eq!(e.map.spawns[1].team, Team::B);

    e.tool = Tool::Pickup;
    e.pointer_press(&down(2.0, 3.0));
    e.tool = Tool::Light;
    e.pointer_press(&down(2.0, 3.0));
    assert_eq!(e.map.pickups.len(), 2);
    assert_eq!(e.map.lights.len(), 2);

    // A zone is drawn like a volume.
    e.symmetry = Symmetry::None;
    e.place.zone = ZoneKind::Water;
    e.tool = Tool::Zone;
    e.pointer_press(&down(10.0, 10.0));
    e.pointer_move(&down(14.0, 14.0));
    e.pointer_release();
    e.pointer_move(&Ray::new(Vec3::new(12.0, 2.0, 40.0), Vec3::NEG_Z));
    e.pointer_press(&down(0.0, 0.0));
    assert!(e.map.brushes.is_empty());
    assert_eq!(e.map.zones.len(), 1);
}

#[test]
fn theme_changes_are_previewed_at_once_and_undoable() {
    let mut e = Editor::blank("theme");
    let mut t = e.map.theme.clone();
    t.ambience = "night".into();
    t.weather = vec![WeatherSlot {
        from_s: 60.0,
        weather: "storm".into(),
    }];
    e.set_theme(t);
    assert!(e.theme_preview(0.0).profile.is_some());
    assert_eq!(e.theme_preview(0.0).weather, "clear");
    assert_eq!(e.theme_preview(90.0).weather, "storm");
    assert!(e.undo());
    assert_eq!(e.map.theme.ambience, "sunset");
}

#[test]
fn panels_run_headless_without_touching_the_document() {
    use faset_ui::{egui, Screen, Theme};
    let ctx = egui::Context::default();
    let mut e = Editor::blank("ui");
    e.set_library(library());
    e.panels.show_all();
    e.tool = Tool::Zone;
    build_full_map(&mut e);
    let before = e.to_ron();
    for _ in 0..3 {
        ctx.run_pass(egui::RawInput::default(), |ctx| {
            e.show(ctx, &Theme::default());
        })
        .drop_without_applying_deltas();
    }
    assert_eq!(e.to_ron(), before);
}
