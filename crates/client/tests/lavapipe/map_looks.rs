//! Every shipped map drawn headless with its own look (ambience, sky,
//! weather, water, lights, props and vegetation) from a spawn point and from
//! above, on Mesa lavapipe. The validation layers must stay silent.
//! `FASET_MAP_SHOTS=<dir>` keeps the captures.

use std::path::{Path, PathBuf};

use faset_client::{load_map, MapLook};
use faset_mapfmt::Team;
use faset_render::glam::Vec3;
use faset_render::{perspective_reverse_z, Camera, RenderConfig, RenderWorld, Renderer};

const SIZE: (u32, u32) = (960, 540);
const MAPS: [&str; 4] = ["driftwood-cove", "red-mesa", "pine-hollow", "rust-harbor"];

fn camera(eye: Vec3, target: Vec3) -> Camera {
    let view = faset_render::glam::camera::rh::view::look_at_mat4(eye, target, Vec3::Y);
    Camera::new(
        view,
        perspective_reverse_z(75f32.to_radians(), SIZE.0 as f32 / SIZE.1 as f32, 0.1),
    )
}

#[test]
fn every_shipped_map_renders_with_its_look() {
    let dir = std::env::var("FASET_MAP_SHOTS").ok().map(PathBuf::from);
    if let Some(dir) = &dir {
        std::fs::create_dir_all(dir).unwrap();
    }
    for name in MAPS {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/maps")
            .join(format!("{name}.ron"));
        let map =
            load_map(&std::fs::read(&path).unwrap()).unwrap_or_else(|e| panic!("{name}: {e}"));
        let look = MapLook::of(&map.map);
        let mut renderer = Renderer::new_headless(&RenderConfig {
            size: SIZE,
            ..RenderConfig::default()
        })
        .expect("headless renderer on lavapipe");
        let instances = map.upload(&mut renderer).expect("the map uploads");
        let spawn = map
            .map
            .spawns
            .iter()
            .find(|s| s.team == Team::A)
            .expect("a team A spawn");
        let feet = Vec3::from_array(spawn.at);
        let forward = Vec3::new(-feet.x, 0.0, -feet.z).normalize_or_zero();
        let mut views = vec![
            (
                "eye".to_owned(),
                feet + Vec3::Y * 1.6,
                feet + Vec3::Y * 1.6 + forward * 20.0,
            ),
            (
                "high".to_owned(),
                Vec3::new(0.0, 55.0, 62.0),
                Vec3::new(0.0, 0.0, 0.0),
            ),
        ];
        // From each border, looking out at the landscape (lot 6.12): a
        // raised camera at the edge of the playable area.
        let s = map.map.surroundings.as_ref().expect("surroundings");
        let [x0, z0, x1, z1] = s.area;
        let up = s.ground + 9.0;
        for (label, eye, out) in [
            ("north", Vec3::new(0.0, up, z0 + 2.0), -Vec3::Z),
            ("east", Vec3::new(x1 - 2.0, up, 0.0), Vec3::X),
            ("south", Vec3::new(0.0, up, z1 - 2.0), Vec3::Z),
            ("west", Vec3::new(x0 + 2.0, up, 0.0), -Vec3::X),
        ] {
            views.push((
                format!("border-{label}"),
                eye,
                eye + out * 60.0 + Vec3::Y * 4.0,
            ));
        }
        for (label, eye, target) in views {
            let mut world = RenderWorld {
                camera: camera(eye, target),
                instances: instances.clone(),
                foliage: true,
                ..RenderWorld::default()
            };
            look.apply(&mut world, 3.0);
            renderer.set_material_time(1.5);
            for _ in 0..3 {
                renderer.render_frame(&world).unwrap();
            }
            let out =
                std::env::temp_dir().join(format!("{}-{name}-{label}.png", std::process::id()));
            renderer.read_back_png(&out).unwrap();
            if let Some(dir) = &dir {
                std::fs::copy(&out, dir.join(format!("{name}-{label}.png"))).unwrap();
            }
            let _ = std::fs::remove_file(out);
        }
        assert_eq!(
            renderer.validation_message_count(),
            0,
            "{name}: validation messages"
        );
    }
}
