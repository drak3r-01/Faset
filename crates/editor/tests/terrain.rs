//! Lot 6.10 acceptance: sculpting the terrain in the editor, undo and redo,
//! and the save round trip.

use faset_core::glam::Vec3;
use faset_editor::{Editor, Ray, SculptOp, Tool};
use faset_mapfmt::geometry::{build_meshes, MeshChunk};
use faset_mapfmt::{expand, parse, Brush, Symmetry};

fn editor() -> Editor {
    let mut e = Editor::blank("hills");
    e.add_brush(Brush::Box {
        min: [-40, -2, -40],
        max: [40, 0, 40],
        material: "concrete".into(),
    });
    e.add_terrain();
    e
}

fn down(x: f32, z: f32) -> Ray {
    Ray::new(Vec3::new(x, 60.0, z), Vec3::NEG_Y)
}

fn chunks_of(e: &Editor) -> Vec<MeshChunk> {
    e.meshes.chunks().cloned().collect()
}

fn height(e: &Editor, x: f32, z: f32) -> f32 {
    e.map.terrain.as_ref().unwrap().height_at(x, z).unwrap()
}

#[test]
fn a_new_terrain_covers_the_brushes() {
    let e = editor();
    let t = e.map.terrain.as_ref().unwrap();
    let (lo, hi) = t.extent_xz();
    assert!(lo[0] <= -20.0 && lo[1] <= -20.0 && hi[0] >= 20.0 && hi[1] >= 20.0);
    assert_eq!(height(&e, 0.0, 0.0), 0.0);
    assert_eq!(e.history.next_undo(), Some("add terrain"));
}

#[test]
fn sculpt_round_trips_through_the_saved_file_and_undoes_exactly() {
    let mut e = editor();
    let mut states = vec![e.to_ron()];
    let ops = [
        (SculptOp::Raise, 0.0, 0.0),
        (SculptOp::Raise, 4.0, 2.0),
        (SculptOp::Lower, -10.0, 6.0),
        (SculptOp::Smooth, 2.0, 1.0),
        (SculptOp::Flatten, 6.0, 0.0),
        (SculptOp::Paint(4), 0.0, 0.0),
    ];
    for (op, x, z) in ops {
        e.set_sculpt_op(op);
        e.sculpt.strength = 1.5;
        e.sculpt_at(x, z);
        let text = e.to_ron();
        assert_ne!(&text, states.last().unwrap(), "{op:?} changed nothing");
        // What is shown is what is saved and read back.
        let back = parse(text.as_bytes()).unwrap();
        assert_eq!(back, e.map);
        assert_eq!(faset_mapfmt::to_ron(&back), text);
        states.push(text);
    }
    assert!(height(&e, 0.0, 0.0) > 1.0);
    assert!(height(&e, -10.0, 6.0) < 0.0);
    assert_eq!(e.history.undo_len(), 2 + ops.len());

    for i in (0..ops.len()).rev() {
        assert!(e.undo());
        assert_eq!(e.to_ron(), states[i], "after undoing sculpt {}", i + 1);
    }
    for state in &states[1..] {
        assert!(e.redo());
        assert_eq!(&e.to_ron(), state);
    }
}

#[test]
fn a_pointer_stroke_is_one_history_step_and_meshes_follow() {
    let mut e = editor();
    e.rebuild_meshes();
    let before = e.to_ron();
    e.tool = Tool::Sculpt;
    e.set_sculpt_op(SculptOp::Raise);
    e.pointer_press(&down(-6.0, 0.0));
    assert!(e.is_sculpting());
    for k in 1..=12 {
        e.pointer_move(&down(-6.0 + k as f32, 0.0));
    }
    // The document shows the stroke while it runs; meshes rebuild per block.
    assert!(height(&e, 0.0, 0.0) > 0.5);
    e.rebuild_meshes();
    assert_eq!(chunks_of(&e), build_meshes(&expand(&e.map)));
    e.pointer_release();
    assert!(!e.is_sculpting());
    assert_eq!(e.history.next_undo(), Some("sculpt (raise)"));
    let after = e.to_ron();

    assert!(e.undo());
    assert_eq!(e.to_ron(), before);
    e.rebuild_meshes();
    assert_eq!(chunks_of(&e), build_meshes(&expand(&e.map)));
    assert!(e.redo());
    assert_eq!(e.to_ron(), after);

    // Escape during a stroke puts the terrain back.
    e.pointer_press(&down(10.0, 10.0));
    e.pointer_move(&down(11.0, 10.0));
    e.cancel();
    assert_eq!(e.to_ron(), after);
}

#[test]
fn symmetry_mirrors_the_dab_and_placement_lands_on_the_terrain() {
    let mut e = editor();
    e.symmetry = Symmetry::Rotate180;
    e.set_sculpt_op(SculptOp::Raise);
    e.sculpt_at(10.0, 6.0);
    let (a, b) = (height(&e, 10.0, 6.0), height(&e, -10.0, -6.0));
    assert!(a > 0.0 && (a - b).abs() < 1e-6, "{a} vs {b}");

    // A spawn placed on the hill stands on its surface.
    e.symmetry = Symmetry::None;
    let p = e.ground_point(&down(10.0, 6.0)).unwrap();
    assert!((p.y - height(&e, p.x, p.z)).abs() < 1e-4, "{p}");
}

#[test]
fn removing_the_terrain_is_undoable() {
    let mut e = editor();
    e.sculpt_at(0.0, 0.0);
    let with = e.to_ron();
    e.remove_terrain();
    assert!(e.map.terrain.is_none());
    assert!(!e.to_ron().contains("terrain"));
    e.undo();
    assert_eq!(e.to_ron(), with);
}
