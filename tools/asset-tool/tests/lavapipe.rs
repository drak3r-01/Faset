//! Thumbnail render tests on lavapipe (run by `cargo xtask render-test`).

use std::path::{Path, PathBuf};

use asset_tool::{contact_sheet, thumbnail, Thumbnailer, THUMB_SIZE};
use faset_assets::{Palette, Recipe};
use image::GenericImageView;

const PROPS: [&str; 2] = ["crate_wood", "tree_pine"];

fn manifest() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn recipe(id: &str) -> Recipe {
    let path = manifest().join(format!("../../assets/props/reference/{id}.ron"));
    Recipe::from_ron(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn reference(id: &str) -> PathBuf {
    manifest().join(format!("tests/refs/{id}.png"))
}

fn render(t: &mut Thumbnailer, id: &str) -> image::RgbaImage {
    thumbnail(t, &recipe(id), &Palette::default()).unwrap()
}

fn max_channel_error(a: &image::RgbaImage, b: &image::RgbaImage) -> u8 {
    assert_eq!(a.dimensions(), b.dimensions(), "image sizes differ");
    a.as_raw()
        .iter()
        .zip(b.as_raw())
        .map(|(x, y)| x.abs_diff(*y))
        .max()
        .unwrap_or(0)
}

#[test]
fn thumbnails_match_references() {
    let mut t = Thumbnailer::new(THUMB_SIZE).unwrap();
    for id in PROPS {
        let got = render(&mut t, id);
        let want = image::open(reference(id)).unwrap().to_rgba8();
        let error = max_channel_error(&got, &want);
        assert!(error <= 1, "{id}: max channel error {error} > 1");
    }
    assert_eq!(t.validation_message_count(), 0, "validation messages");
}

#[test]
fn contact_sheet_tiles_thumbnails() {
    let mut t = Thumbnailer::new(THUMB_SIZE).unwrap();
    let thumbs: Vec<_> = PROPS.iter().map(|id| render(&mut t, id)).collect();
    let sheet = contact_sheet(&thumbs, 5);
    assert_eq!(sheet.dimensions(), (2 * THUMB_SIZE, THUMB_SIZE));
    assert_eq!(
        sheet.view(0, 0, THUMB_SIZE, THUMB_SIZE).to_image(),
        thumbs[0]
    );
    assert_eq!(t.validation_message_count(), 0, "validation messages");
}

/// Regenerates `tests/refs/*.png`. Inspect the images before committing.
#[test]
#[ignore = "regenerates tests/refs"]
fn bless_references() {
    let mut t = Thumbnailer::new(THUMB_SIZE).unwrap();
    for id in PROPS {
        render(&mut t, id).save(reference(id)).unwrap();
    }
}
