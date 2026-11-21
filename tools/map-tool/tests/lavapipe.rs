//! Top and perspective views of the example map on Mesa lavapipe (run by
//! `cargo xtask render-test`), against reference images.

use std::path::{Path, PathBuf};

use faset_client::load_map;
use image::RgbaImage;
use map_tool::view::{Viewer, VIEW_SIZE};

const EXAMPLE: &[u8] = include_bytes!("../../../assets/maps/example.ron");

fn reference(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/refs/{name}.png"))
}

fn render() -> (RgbaImage, RgbaImage, u32) {
    let map = load_map(EXAMPLE).expect("example map loads");
    let mut viewer = Viewer::new(&map, VIEW_SIZE).expect("headless renderer on lavapipe");
    let top = viewer.top(&map).unwrap();
    let perspective = viewer.perspective().unwrap();
    (top, perspective, viewer.validation_message_count())
}

fn compare(name: &str, got: &RgbaImage) {
    let reference = image::open(reference(name))
        .expect("reference image")
        .to_rgba8();
    assert_eq!(got.dimensions(), reference.dimensions());
    let error = got
        .as_raw()
        .iter()
        .zip(reference.as_raw())
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(0);
    assert!(error <= 1, "{name}: max channel error {error} > 1");
    let clear = got.get_pixel(0, 0).0;
    let drawn = got.pixels().filter(|p| p.0 != clear).count();
    assert!(drawn > got.pixels().len() / 4, "{name} is nearly blank");
}

#[test]
fn views_match_references() {
    let (top, perspective, messages) = render();
    assert_eq!(messages, 0, "validation messages");
    compare("example_top", &top);
    compare("example_perspective", &perspective);
}

/// Regenerates the references from a lavapipe render. Inspect before committing.
#[test]
#[ignore = "regenerates tests/refs/example_*.png"]
fn bless_view_references() {
    let (top, perspective, _) = render();
    std::fs::create_dir_all(reference("x").parent().unwrap()).unwrap();
    top.save(reference("example_top")).unwrap();
    perspective.save(reference("example_perspective")).unwrap();
}
