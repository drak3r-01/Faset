// Weather particles (rain, snow, sand): instanced camera-facing quads with no
// per-particle CPU data. Mirrors `render::weather`.
//
// A particle's position is computed here from its index (instance index),
// the time and a box that follows the camera:
//   p_i = fract(h_i + v t / B - o / B) B + o,   o = O_cam - B / 2
// where h_i is a per-particle hash, v the velocity (fall plus wind), t the
// time and B the box side. The box is centred on the camera and the field is
// fixed in the world. Rain and sand quads are stretched along v; snow adds a
// sinusoidal sway to the fall. A particle under a roof (the shelter height map
// is higher than the particle) is collapsed and not drawn.
//
// The push constant `kind` selects rain (0), snow (1) or sand (2); the CPU
// draws as many instances as the intensity asks for.

#include "noise.wgsl"
#include "lighting.wgsl"

struct Immediates {
    // Sub-pixel jitter in NDC units (see `opaque.wgsl`).
    jitter: vec2<f32>,
    // Animation time in seconds.
    time: f32,
    // Particle kind as a float: 0 rain, 1 snow, 2 sand.
    kind: f32,
}

var<immediate> imm: Immediates;

const TAU: f32 = 6.2831853;

// Must match `render::weather`.
const KIND_RAIN: u32 = 0u;
const KIND_SNOW: u32 = 1u;
const SNOW_SWAY: f32 = 0.35;

// Same bits as `render::weather::particle_hash`.
fn particle_hash(index: u32, axis: u32) -> f32 {
    let h = hash_u32(index ^ hash_u32(axis + 0x68bc21ebu));
    return f32(h >> 8u) * (1.0 / 16777215.0);
}

fn box_size(kind: u32) -> f32 {
    if kind == KIND_SNOW {
        return 18.0;
    }
    return 24.0;
}

// Fall plus wind, in metres per second.
fn particle_velocity(kind: u32, wind: vec3<f32>) -> vec3<f32> {
    if kind == KIND_RAIN {
        return vec3<f32>(wind.x, -9.0 + wind.y, wind.z);
    }
    if kind == KIND_SNOW {
        return vec3<f32>(0.6 * wind.x, -1.3 + 0.6 * wind.y, 0.6 * wind.z);
    }
    let flat_wind = vec2<f32>(wind.x, wind.z);
    var dir = vec2<f32>(1.0, 0.0);
    let speed = length(flat_wind);
    if speed > 1.0e-4 {
        dir = flat_wind / speed;
    }
    return vec3<f32>(dir.x * max(speed, 6.0), -0.4 + wind.y, dir.y * max(speed, 6.0));
}

fn snow_sway(index: u32, t: f32) -> vec3<f32> {
    let a = particle_hash(index, 3u);
    let b = particle_hash(index, 4u);
    let c = particle_hash(index, 5u);
    let freq = 0.8 + 1.2 * c;
    return vec3<f32>(
        SNOW_SWAY * sin(t * freq + TAU * a),
        0.0,
        SNOW_SWAY * cos(t * (freq * 0.83) + TAU * b),
    );
}

// True when the shelter map has a surface above `p`.
fn sheltered(p: vec3<f32>) -> bool {
    if lighting.shelter_origin.w < 0.5 {
        return false;
    }
    let cell = floor((p.xz - lighting.shelter_origin.xy) / lighting.shelter_origin.z);
    let dims = lighting.shelter_dims.xy;
    if cell.x < 0.0 || cell.y < 0.0 || cell.x >= dims.x || cell.y >= dims.y {
        return false;
    }
    let i = u32(cell.y) * u32(dims.x) + u32(cell.x);
    return shelter_heights[i] > p.y;
}

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    // Quad coordinates in [-1, 1]; y points along the velocity.
    @location(0) uv: vec2<f32>,
    @location(1) world_pos: vec3<f32>,
    // Opacity from fading and thinness.
    @location(2) alpha: f32,
}

fn corner_of(vertex: u32) -> vec2<f32> {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, 1.0),
    );
    return corners[vertex];
}

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32, @builtin(instance_index) index: u32) -> VertexOut {
    let kind = u32(imm.kind + 0.5);
    let t = imm.time;
    let cam = lighting.camera_pos.xyz;
    let b = box_size(kind);
    let v = particle_velocity(kind, lighting.weather_wind.xyz);
    let h = vec3<f32>(particle_hash(index, 0u), particle_hash(index, 1u), particle_hash(index, 2u));
    let o = cam - vec3<f32>(0.5 * b);
    let u = h + v * (t / b) - o / b;
    var p = (u - floor(u)) * b + o;
    if kind == KIND_SNOW {
        p += snow_sway(index, t);
    }

    var out: VertexOut;
    out.uv = vec2<f32>(0.0);
    out.world_pos = p;
    out.alpha = 0.0;
    // Collapsed: outside the clip volume, so the triangle is culled.
    out.position = vec4<f32>(0.0, 0.0, 2.0, 1.0);
    if sheltered(p) {
        return out;
    }

    let to_cam = cam - p;
    let dist = max(length(to_cam), 1.0e-3);
    let view = to_cam / dist;
    let corner = corner_of(vertex);

    // Fade at the edge of the box (where particles wrap) and very close to
    // the camera.
    let edge = max(abs(p.x - cam.x), max(abs(p.y - cam.y), abs(p.z - cam.z))) / (0.5 * b);
    let fade = (1.0 - smoothstep(0.7, 1.0, edge)) * smoothstep(0.3, 1.5, dist);

    var world = p;
    var width = 0.0;
    if kind == KIND_SNOW {
        let right = normalize(cross(vec3<f32>(0.0, 1.0, 0.0), view) + vec3<f32>(1.0e-6, 0.0, 0.0));
        let up = cross(view, right);
        let radius = 0.025 + 0.0025 * dist;
        world = p + (right * corner.x + up * corner.y) * radius;
        width = radius;
    } else {
        // A streak along the velocity, facing the camera.
        let speed = length(v);
        let axis = v / max(speed, 1.0e-4);
        var side = cross(axis, view);
        let side_len = length(side);
        if side_len < 1.0e-4 {
            side = vec3<f32>(1.0, 0.0, 0.0);
        } else {
            side = side / side_len;
        }
        if kind == KIND_RAIN {
            width = max(0.010, 0.0035 * dist);
            world = p + side * (corner.x * width) + axis * (corner.y * 0.5 * 0.04 * speed);
        } else {
            width = max(0.018, 0.004 * dist);
            world = p + side * (corner.x * width) + axis * (corner.y * 0.5 * 0.07 * speed);
        }
    }
    // Streaks thinner than a pixel are widened and made fainter.
    var thin = 1.0;
    if kind == KIND_RAIN {
        thin = sqrt(min(1.0, 0.010 / width));
    } else if kind != KIND_SNOW {
        thin = sqrt(min(1.0, 0.018 / width));
    }

    let clip = lighting.clip_from_world * vec4<f32>(world, 1.0);
    out.position = vec4<f32>(clip.xy + imm.jitter * clip.w, clip.zw);
    out.uv = corner;
    out.world_pos = world;
    out.alpha = fade * thin;
    return out;
}


@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let kind = u32(imm.kind + 0.5);
    // Light reaching the particles: ambient plus some sun, never fully dark.
    let env = 0.5 * (lighting.ambient_sky.rgb + lighting.ambient_ground.rgb)
        + lighting.sun_color.rgb * 0.45 + vec3<f32>(0.03);
    var colour = vec3<f32>(0.0);
    var alpha = in.alpha;
    if kind == KIND_RAIN {
        let across = 1.0 - smoothstep(0.35, 1.0, abs(in.uv.x));
        // Brighter at the head of the streak, faint tail.
        let head = 0.3 + 0.7 * (0.5 * in.uv.y + 0.5);
        alpha *= 0.5 * across * head;
        colour = vec3<f32>(0.78, 0.85, 1.0) * env * 1.6;
    } else if kind == KIND_SNOW {
        let round = 1.0 - smoothstep(0.55, 1.0, length(in.uv));
        alpha *= 0.9 * round;
        colour = vec3<f32>(0.95, 0.97, 1.0) * env * 1.8;
    } else {
        let across = 1.0 - smoothstep(0.35, 1.0, abs(in.uv.x));
        let along = 1.0 - smoothstep(0.5, 1.0, abs(in.uv.y));
        alpha *= 0.6 * across * along;
        colour = vec3<f32>(0.78, 0.60, 0.36) * env * 1.4;
    }
    return vec4<f32>(apply_fog(colour, in.world_pos), alpha);
}
