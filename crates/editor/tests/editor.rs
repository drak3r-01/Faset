//! Acceptance tests of the editor core: undo of a built room, incremental
//! mesh rebuild, and gestures driven through pointer events.

use faset_core::glam::{Vec2, Vec3};
use faset_editor::{
    AddBrush, AddProp, BrushKind, CameraMode, Editor, HandleMode, Ray, RemoveBrush, ReplaceBrush,
    Selection, Tool,
};
use faset_mapfmt::geometry::{build_meshes, MeshChunk};
use faset_mapfmt::{expand, parse, Brush, Prop};

fn boxed(min: [i32; 3], max: [i32; 3]) -> Brush {
    Brush::Box {
        min,
        max,
        material: "concrete".into(),
    }
}

fn down(x: f32, z: f32) -> Ray {
    Ray::new(Vec3::new(x, 60.0, z), Vec3::NEG_Y)
}

fn chunks_of(editor: &Editor) -> Vec<MeshChunk> {
    editor.meshes.chunks().cloned().collect()
}

fn full_build(editor: &Editor) -> Vec<MeshChunk> {
    build_meshes(&expand(&editor.map))
}

#[test]
fn a_room_is_built_then_undone_command_by_command() {
    let mut editor = Editor::blank("room");
    let mut states = vec![editor.to_ron()];
    let commands: Vec<Box<dyn faset_editor::Command>> = vec![
        Box::new(AddBrush::new(boxed([-20, -2, -20], [20, 0, 20]))), // floor
        Box::new(AddBrush::new(boxed([-20, 0, -20], [20, 12, -18]))), // north wall
        Box::new(AddBrush::new(boxed([-20, 0, 18], [20, 12, 20]))),  // south wall
        Box::new(AddBrush::new(boxed([-20, 0, -18], [-18, 12, 18]))), // west wall
        Box::new(AddProp::new(Prop {
            recipe: "barrel_oil".into(),
            at: [1.0, 0.0, 2.5],
            yaw: 30.0,
            scale: 1.0,
        })),
        Box::new(ReplaceBrush::new(
            1,
            boxed([-20, 0, -20], [20, 12, -18]),
            boxed([-20, 0, -20], [20, 16, -18]),
        )),
        Box::new(RemoveBrush::new(2, boxed([-20, 0, 18], [20, 12, 20]))),
    ];
    for command in commands {
        editor.execute(command);
        // Every state is a valid, loadable map, saved exactly as shown.
        let text = editor.to_ron();
        assert_eq!(parse(text.as_bytes()).unwrap(), editor.map);
        states.push(text);
    }
    assert_eq!(editor.map.brushes.len(), 3);
    assert_eq!(editor.history.undo_len(), 7);

    for i in (0..7).rev() {
        assert!(editor.undo());
        assert_eq!(
            editor.to_ron(),
            states[i],
            "after undoing command {}",
            i + 1
        );
    }
    assert!(!editor.undo(), "history is empty");

    // Redo replays the same states.
    for state in &states[1..] {
        assert!(editor.redo());
        assert_eq!(&editor.to_ron(), state);
    }
    assert!(!editor.redo());
}

#[test]
fn a_new_command_drops_the_redo_stack() {
    let mut editor = Editor::blank("t");
    editor.add_brush(boxed([0, 0, 0], [4, 4, 4]));
    editor.undo();
    assert_eq!(editor.history.redo_len(), 1);
    editor.add_brush(boxed([8, 0, 0], [12, 4, 4]));
    assert_eq!(editor.history.redo_len(), 0);
    assert!(!editor.redo());
}

#[test]
fn a_single_brush_edit_rebuilds_only_the_blocks_it_touches() {
    let mut editor = Editor::blank("blocks");
    // Four brushes spread over four distinct 16 m blocks (the first three
    // along x, the last one above).
    for i in 0..3 {
        let x = i * 40;
        editor.add_brush(boxed([x + 4, 4, 4], [x + 12, 10, 12]));
    }
    editor.add_brush(boxed([4, 40, 4], [12, 46, 12]));
    let first = editor.rebuild_meshes();
    assert_eq!(first.len(), 4, "first build covers the whole map");
    assert_eq!(chunks_of(&editor), full_build(&editor));
    let total = editor.meshes.rebuilt_total();

    // A brush inside one block (2 m cube well inside block [0, 0, 0]).
    editor.add_brush(boxed([20, 4, 20], [24, 8, 24]));
    let rebuilt = editor.rebuild_meshes();
    assert_eq!(rebuilt, vec![[0, 0, 0]]);
    assert_eq!(editor.meshes.last_rebuilt(), &[[0, 0, 0]]);
    assert_eq!(editor.meshes.rebuilt_total(), total + 1);
    assert_eq!(chunks_of(&editor), full_build(&editor));

    // Undo rebuilds that one block again and matches the full build.
    editor.undo();
    assert_eq!(editor.rebuild_meshes(), vec![[0, 0, 0]]);
    assert_eq!(chunks_of(&editor), full_build(&editor));

    // Nothing changed: nothing rebuilt.
    assert!(editor.rebuild_meshes().is_empty());

    // A brush straddling the block plane x = 16 m touches both blocks.
    editor.add_brush(boxed([28, 4, 4], [36, 8, 8]));
    let mut rebuilt = editor.rebuild_meshes();
    rebuilt.sort_unstable();
    assert_eq!(rebuilt, vec![[0, 0, 0], [1, 0, 0]]);
    assert_eq!(chunks_of(&editor), full_build(&editor));

    // Editing a brush that touches another one regenerates the shared face
    // exactly as a full build does.
    editor.add_brush(boxed([36, 4, 4], [44, 8, 8]));
    editor.rebuild_meshes();
    editor.undo();
    editor.rebuild_meshes();
    assert_eq!(chunks_of(&editor), full_build(&editor));
}

#[test]
fn props_do_not_rebuild_meshes() {
    let mut editor = Editor::blank("t");
    editor.add_brush(boxed([0, 0, 0], [4, 4, 4]));
    editor.rebuild_meshes();
    editor.add_prop(Prop {
        recipe: "crate".into(),
        at: [1.0, 0.0, 1.0],
        yaw: 0.0,
        scale: 1.0,
    });
    assert!(editor.rebuild_meshes().is_empty());
}

#[test]
fn drawing_a_volume_with_the_mouse_creates_one_undoable_command() {
    let mut editor = Editor::blank("t");
    editor.tool = Tool::Create;
    editor.creation.settings.kind = BrushKind::Stairs;
    editor.pointer_press(&down(1.0, 1.0));
    editor.pointer_move(&down(4.0, 5.0));
    editor.pointer_release();
    // Pull the height with a ray from the side at 2 m.
    editor.pointer_move(&Ray::new(Vec3::new(3.0, 2.0, 40.0), Vec3::NEG_Z));
    assert!(editor.map.brushes.is_empty(), "nothing is committed yet");
    editor.pointer_press(&down(0.0, 0.0));
    assert_eq!(editor.map.brushes.len(), 1);
    let Brush::Stairs {
        min, max, steps, ..
    } = &editor.map.brushes[0]
    else {
        panic!("stairs expected")
    };
    assert_eq!((*min, *max), ([2, 0, 2], [8, 4, 10]));
    assert_eq!(*steps, 8);
    assert_eq!(editor.history.undo_len(), 1);
    editor.undo();
    assert!(editor.map.brushes.is_empty());
}

#[test]
fn escape_drops_a_volume_being_drawn() {
    let mut editor = Editor::blank("t");
    editor.tool = Tool::Create;
    editor.pointer_press(&down(0.0, 0.0));
    editor.pointer_move(&down(4.0, 4.0));
    editor.cancel();
    editor.pointer_release();
    assert!(editor.map.brushes.is_empty());
}

#[test]
fn the_top_view_draws_volumes_with_the_default_height() {
    let mut editor = Editor::blank("t");
    editor.camera.mode = CameraMode::TopOrtho;
    editor.camera.top_centre = Vec2::ZERO;
    editor.tool = Tool::Create;
    let size = Vec2::new(1600.0, 900.0);
    let ray = |x: f32, z: f32| {
        // Pixel of a ground point in the top view.
        let half = editor.camera.top_half_height;
        let px = Vec2::new(
            size.x * 0.5 + x / (half * size.x / size.y) * size.x * 0.5,
            size.y * 0.5 + z / half * size.y * 0.5,
        );
        editor.camera.ray_from_pixel(px, size)
    };
    let (a, b) = (ray(2.0, 2.0), ray(6.0, 5.0));
    editor.pointer_press(&a);
    editor.pointer_move(&b);
    editor.pointer_release();
    let Brush::Box { min, max, .. } = &editor.map.brushes[0] else {
        panic!("box expected")
    };
    assert_eq!((*min, *max), ([4, 0, 4], [12, 6, 10]));
}

#[test]
fn selecting_and_dragging_a_handle_is_one_command_and_cancel_restores() {
    let mut editor = Editor::blank("t");
    editor.add_brush(boxed([0, 0, 0], [4, 4, 4]));
    editor.add_brush(boxed([20, 0, 0], [24, 4, 4]));
    editor.selection = None;
    let before = editor.to_ron();

    // Pick the second brush with a ray from the side.
    let side = |x: f32, y: f32| Ray::new(Vec3::new(x, y, 30.0), Vec3::NEG_Z);
    editor.pointer_press(&side(11.0, 1.0));
    editor.pointer_release();
    assert_eq!(editor.selection, Some(Selection::Brush(1)));

    // Drag its X arrow (centre (11, 1, 1) m, arrow along +x) by 2.9 m.
    editor.handle_mode = HandleMode::Move;
    let centre = editor.selection_centre().unwrap();
    editor.pointer_press(&side(centre.x + 0.5, centre.y));
    assert!(editor.is_dragging());
    editor.pointer_move(&side(centre.x + 3.4, centre.y));
    // The document shows the preview but the history is untouched.
    assert_eq!(editor.history.undo_len(), 2);
    assert_eq!(
        editor.map.brushes[1],
        boxed([26, 0, 0], [30, 4, 4]),
        "preview is written to the document"
    );
    editor.cancel();
    assert_eq!(editor.to_ron(), before);

    editor.pointer_press(&side(centre.x + 0.5, centre.y));
    editor.pointer_move(&side(centre.x + 3.4, centre.y));
    editor.pointer_release();
    assert_eq!(editor.history.undo_len(), 3);
    assert_eq!(editor.map.brushes[1], boxed([26, 0, 0], [30, 4, 4]));
    editor.undo();
    assert_eq!(editor.to_ron(), before);
}

#[test]
fn deleting_the_selection_is_undoable() {
    let mut editor = Editor::blank("t");
    editor.add_brush(boxed([0, 0, 0], [4, 4, 4]));
    editor.add_brush(boxed([8, 0, 0], [12, 4, 4]));
    let before = editor.to_ron();
    editor.selection = Some(Selection::Brush(0));
    editor.delete_selection();
    assert_eq!(editor.map.brushes.len(), 1);
    assert_eq!(editor.selection, None);
    editor.undo();
    assert_eq!(editor.to_ron(), before);
}
