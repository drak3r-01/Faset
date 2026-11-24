// Instanced vegetation (lot 6.11): every plant of a map is one 32-byte
// instance (base, yaw, scale, sway, fade distance, wind phase) in a static
// buffer; the vertex shader builds its transform, bends it in the wind and
// projects it with the camera of the lighting block. The fragment is shaded
// like any opaque surface (`surface.wgsl`) and dissolves with an ordered
// dither over the last FADE_BAND of its draw distance. `vs_shadow` draws the
// same plants into a sun shadow cascade. Mirrors `render::foliage`.

#include "surface.wgsl"

struct Immediates {
    // Sub-pixel jitter in NDC units (see opaque.wgsl).
    jitter: vec2<f32>,
    // Material and wind time in seconds.
    time: f32,
    // Shadow pass: index of the cascade being drawn.
    cascade: f32,
}

var<immediate> imm: Immediates;

// Share of the draw distance over which a plant dissolves.
const FADE_BAND: f32 = 0.2;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec4<f32>,
    @location(2) color: vec4<f32>,
}

struct PlantIn {
    // xyz base of the plant, w yaw in radians.
    @location(3) base_yaw: vec4<f32>,
    // x scale, y sway, z fade end distance, w wind phase.
    @location(4) params: vec4<f32>,
}

struct PlantOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) clip: vec4<f32>,
    @location(3) prev_clip: vec4<f32>,
    @location(4) world_pos: vec3<f32>,
    @location(5) @interpolate(flat) material: u32,
    @location(6) @interpolate(flat) fade_end: f32,
}

// Horizontal wind offset of a point `height` metres above the base of a plant
// (`render::foliage::sway_offset`).
fn sway_offset(height: f32, sway: f32, phase: f32, time: f32, wind: vec2<f32>) -> vec2<f32> {
    let speed = length(wind);
    var dir = vec2<f32>(0.70710677, 0.70710677);
    if speed > 0.01 {
        dir = wind / speed;
    }
    let strength = 1.0 + min(speed, 20.0) * 0.15;
    let wave = 0.6 * sin(time * 1.7 + phase) + 0.25 * sin(time * 3.1 + phase * 1.3) + 0.35;
    return dir * (sway * strength * wave * height * height);
}

// Rotation about +Y by `yaw` (glam's `Mat3::from_rotation_y`).
fn turn(v: vec3<f32>, yaw: f32) -> vec3<f32> {
    let c = cos(yaw);
    let s = sin(yaw);
    return vec3<f32>(c * v.x + s * v.z, v.y, -s * v.x + c * v.z);
}

fn plant_world(local: vec3<f32>, p: PlantIn) -> vec3<f32> {
    let q = local * p.params.x;
    let off = sway_offset(max(q.y, 0.0), p.params.y, p.params.w, imm.time, lighting.weather_wind.xz);
    return p.base_yaw.xyz + turn(q, p.base_yaw.w) + vec3<f32>(off.x, 0.0, off.y);
}

@vertex
fn vs_main(v: VertexIn, p: PlantIn) -> PlantOut {
    let world = vec4<f32>(plant_world(v.position, p), 1.0);
    let clip = lighting.clip_from_world * world;
    var out: PlantOut;
    out.position = vec4<f32>(clip.xy + imm.jitter * clip.w, clip.zw);
    out.color = v.color.rgb;
    out.normal = turn(v.normal.xyz, p.base_yaw.w);
    out.clip = clip;
    // The sway of the previous frame is not tracked: the motion is the
    // camera's only.
    out.prev_clip = lighting.sky_prev_view_proj * world;
    out.world_pos = world.xyz;
    out.material = u32(round(v.color.a * 255.0));
    out.fade_end = p.params.z;
    return out;
}

@vertex
fn vs_shadow(v: VertexIn, p: PlantIn) -> @builtin(position) vec4<f32> {
    let world = vec4<f32>(plant_world(v.position, p), 1.0);
    return lighting.shadow_matrices[u32(imm.cascade)] * world;
}

// 4 x 4 ordered dither threshold in (0, 1).
fn bayer4(pixel: vec2<f32>) -> f32 {
    var m = array<f32, 16>(0.0, 8.0, 2.0, 10.0, 12.0, 4.0, 14.0, 6.0, 3.0, 11.0, 1.0, 9.0, 15.0, 7.0, 13.0, 5.0);
    let x = u32(pixel.x) & 3u;
    let y = u32(pixel.y) & 3u;
    return (m[y * 4u + x] + 0.5) / 16.0;
}

@fragment
fn fs_main(in: PlantOut) -> FragmentOut {
    let out = shade_fragment(in.color, in.normal, in.clip, in.prev_clip, in.world_pos, in.material);
    let d = distance(in.world_pos, lighting.camera_pos.xyz);
    let fade = clamp((in.fade_end - d) / (FADE_BAND * in.fade_end), 0.0, 1.0);
    if fade < bayer4(in.position.xy) {
        discard;
    }
    return out;
}
