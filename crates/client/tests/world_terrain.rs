//! Lot 8.1 acceptance on the proof map, Pine Hollow in its 1 km world:
//! the map stays small enough to transfer, its playable ground is the
//! simulated terrain, and streaming its chunks stays bounded in memory and
//! in time per frame while the eye walks from a spawn to the border and
//! turns around.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use faset_client::world_terrain::{
    ChunkSink, CHUNK_BYTES, MAX_BUILDS_PER_FRAME, SLOT_COUNT, SLOT_INDICES, SLOT_VERTICES,
};
use faset_client::{load_map, TerrainStream};
use faset_mapfmt::Team;
use faset_render::glam::{Mat4, Vec3};
use faset_render::{perspective_reverse_z, MeshId, RenderError, Vertex, STREAM_BYTES_PER_FRAME};

fn bytes() -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/maps/pine-hollow.ron");
    std::fs::read(path).unwrap()
}

/// Accepts writes within the renderer's budget, like `Renderer`.
struct Recorder {
    left: usize,
    slots: BTreeSet<MeshId>,
    writes: usize,
}

impl ChunkSink for Recorder {
    fn bytes_left(&self) -> usize {
        self.left
    }

    fn write(&mut self, slot: MeshId, v: &[Vertex], i: &[u32]) -> Result<(), RenderError> {
        assert!(v.len() <= SLOT_VERTICES as usize && i.len() <= SLOT_INDICES as usize);
        let bytes = v.len() * 20 + i.len() * 4;
        if bytes > self.left {
            return Err(RenderError::StreamBudget {
                bytes,
                left: self.left,
            });
        }
        self.left -= bytes;
        self.slots.insert(slot);
        self.writes += 1;
        Ok(())
    }
}

fn view_proj(eye: Vec3, yaw: f32) -> Mat4 {
    let dir = Vec3::new(yaw.sin(), -0.1, -yaw.cos());
    let view = faset_render::glam::camera::rh::view::look_at_mat4(eye, eye + dir, Vec3::Y);
    perspective_reverse_z(75f32.to_radians(), 16.0 / 9.0, 0.1) * view
}

#[test]
fn pine_hollow_sits_in_a_one_kilometre_world() {
    let raw = bytes();
    // Map transfer size (decision of lot 8.1): the far field adds about
    // 28 KB; the whole file stays far under the format's 256 KiB.
    println!("pine-hollow.ron: {} bytes", raw.len());
    assert!(raw.len() < 96 * 1024, "{} bytes", raw.len());
    let loaded = load_map(&raw).unwrap();
    let world = loaded.map.world.as_ref().expect("a world block");
    let [x0, z0, x1, z1] = world.extent;
    assert!(x1 - x0 >= 1000.0 && z1 - z0 >= 1000.0);
    let terrain = loaded.world.as_ref().expect("a streamed world terrain");
    // The playable ground is the simulated terrain: same grid, same heights.
    let t = loaded.map.terrain.as_ref().unwrap();
    assert_eq!(terrain.cell_m(), t.cell_m());
    for (i, j) in [(0, 0), (10, 7), (26, 18), (52, 36)] {
        let p = t.sample_pos(i, j);
        assert!((world.height_at(Some(t), p.x, p.z) - p.y).abs() < 1e-5);
    }
    // The far landscape rises into mountains around the valley.
    let far = world.height_at(Some(t), 0.0, -400.0);
    assert!(far > 100.0, "{far}");
}

#[test]
fn streaming_pine_hollow_is_bounded_in_memory_and_time() {
    let loaded = load_map(&bytes()).unwrap();
    let terrain = Arc::clone(loaded.world.as_ref().unwrap());
    let slots: Vec<MeshId> = (0..SLOT_COUNT).map(|k| MeshId(1000 + k)).collect();
    let mut stream = TerrainStream::new(terrain, slots);
    let mut sink = Recorder {
        left: 0,
        slots: BTreeSet::new(),
        writes: 0,
    };
    let spawn = loaded
        .map
        .spawns
        .iter()
        .find(|s| s.team == Team::A)
        .unwrap();
    let start = Vec3::from_array(spawn.at) + Vec3::Y * 1.6;
    let border = Vec3::new(62.0, 10.0, 0.0);
    let mut worst = 0u64;
    let mut settled = 0;
    for frame in 0..900 {
        // Walk to the east border in 600 frames, then spin on the spot.
        let t = (frame as f32 / 600.0).min(1.0);
        let eye = start.lerp(border, t);
        let yaw = if frame < 600 {
            1.57
        } else {
            frame as f32 * 0.25
        };
        sink.left = STREAM_BYTES_PER_FRAME;
        let stats = stream.update(eye, &view_proj(eye, yaw), &mut sink).unwrap();
        assert!(stats.built <= MAX_BUILDS_PER_FRAME, "frame {frame}");
        assert!(stats.bytes <= MAX_BUILDS_PER_FRAME * CHUNK_BYTES);
        assert!(stats.resident <= SLOT_COUNT as usize);
        worst = worst.max(stats.micros);
        if frame > 0 {
            assert!(stats.drawn > 0, "frame {frame}: nothing drawn");
        }
        if frame >= 620 {
            // Settled at the border, turning loads nothing.
            if stream.is_settled() {
                settled += 1;
                assert_eq!(stats.built, 0, "frame {frame}");
            }
        }
    }
    println!(
        "pine-hollow streaming: {} writes into {} slots, worst update {worst} us, last {:?}",
        sink.writes,
        sink.slots.len(),
        stream.last_stats()
    );
    assert!(settled > 200, "settled {settled} frames while turning");
    assert!(sink.slots.len() <= SLOT_COUNT as usize);
    // Debug builds get a loose bound that still catches a regression; an
    // optimised build stays well under a millisecond.
    let limit = if cfg!(debug_assertions) {
        300_000
    } else {
        2_000
    };
    assert!(worst < limit, "worst update {worst} us");
}
