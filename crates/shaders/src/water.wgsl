// Water surface: a grid around the camera displaced by a sum of Gerstner
// waves. Mirrors `render::water`.
//
//   phi_i = k_i D_i . x0 - omega_i t
//   x += sum Q_i A_i D_i cos(phi_i)     y += sum A_i sin(phi_i)
//
// The grid is generated from the vertex index (no vertex buffer); its cells
// grow with the distance from the camera (offset = R u |u|). The normal comes
// from the analytic partial derivatives of the sum. Seen from above the
// colour is a Fresnel mix of the water body (shallow to deep) and the sky
// gradient plus a sun glint; seen from below it is the lit, fogged
// underside, with the underwater fog replacing the height fog. The surface
// also writes the motion vectors (camera motion only) and the normals, like
// the opaque pass, so temporal anti-aliasing and the contours see it.

#include "noise.wgsl"
#include "lighting.wgsl"

struct Immediates {
    jitter: vec2<f32>,
    time: f32,
    _pad: f32,
}

var<immediate> imm: Immediates;

// Must match `render::water`.
const GRID_CELLS: u32 = 160u;
const GRID_EXTENT: f32 = 300.0;

struct Surface {
    position: vec3<f32>,
    normal: vec3<f32>,
}

fn gerstner(p: vec2<f32>, t: f32) -> Surface {
    var pos = vec3<f32>(p.x, 0.0, p.y);
    var dxx = 0.0;
    var dxz = 0.0;
    var dzz = 0.0;
    var hx = 0.0;
    var hz = 0.0;
    let count = u32(lighting.water_params.w);
    for (var i = 0u; i < count; i++) {
        let dir = lighting.water_wave_dir[i];
        let amp = lighting.water_wave_amp[i];
        let d = dir.xy;
        let phase = dir.z * dot(d, p) - dir.w * t;
        let s = sin(phase);
        let c = cos(phase);
        pos.x += amp.y * amp.x * d.x * c;
        pos.z += amp.y * amp.x * d.y * c;
        pos.y += amp.x * s;
        let qak = amp.y * amp.x * dir.z;
        dxx += qak * d.x * d.x * s;
        dxz += qak * d.x * d.y * s;
        dzz += qak * d.y * d.y * s;
        hx += amp.x * dir.z * d.x * c;
        hz += amp.x * dir.z * d.y * c;
    }
    let tx = vec3<f32>(1.0 - dxx, hx, -dxz);
    let tz = vec3<f32>(-dxz, hz, 1.0 - dzz);
    var out: Surface;
    out.position = pos;
    out.normal = normalize(cross(tz, tx));
    return out;
}

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    // Unjittered clip positions of this frame and the previous one (the
    // surface is taken as static in the world: camera motion only).
    @location(2) clip: vec4<f32>,
    @location(3) prev_clip: vec4<f32>,
}

fn grid_coord(i: u32) -> f32 {
    let u = f32(i) / f32(GRID_CELLS) * 2.0 - 1.0;
    return GRID_EXTENT * u * abs(u);
}

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32) -> VertexOut {
    var corners = array<vec2<u32>, 6>(
        vec2<u32>(0u, 0u),
        vec2<u32>(1u, 0u),
        vec2<u32>(1u, 1u),
        vec2<u32>(0u, 0u),
        vec2<u32>(1u, 1u),
        vec2<u32>(0u, 1u),
    );
    let quad = vertex / 6u;
    let corner = corners[vertex % 6u];
    let ix = quad % GRID_CELLS + corner.x;
    let iz = quad / GRID_CELLS + corner.y;
    let cam = lighting.camera_pos.xyz;
    let rest = vec2<f32>(cam.x + grid_coord(ix), cam.z + grid_coord(iz));
    let s = gerstner(rest, imm.time);
    let world = vec3<f32>(s.position.x, lighting.water_params.x + s.position.y, s.position.z);

    var out: VertexOut;
    let clip = lighting.clip_from_world * vec4<f32>(world, 1.0);
    out.position = vec4<f32>(clip.xy + imm.jitter * clip.w, clip.zw);
    out.world_pos = world;
    out.normal = s.normal;
    out.clip = clip;
    out.prev_clip = lighting.sky_prev_view_proj * vec4<f32>(world, 1.0);
    return out;
}

struct FragmentOut {
    @location(0) color: vec4<f32>,
    @location(1) motion: vec2<f32>,
    @location(2) normal: vec2<f32>,
}

// Octahedral encoding of a unit vector into [0, 1]^2 (as in `opaque.wgsl`).
fn encode_octahedral(n: vec3<f32>) -> vec2<f32> {
    let p = n.xy / (abs(n.x) + abs(n.y) + abs(n.z));
    var e = p;
    if n.z < 0.0 {
        e = (1.0 - abs(p.yx)) * select(vec2<f32>(-1.0), vec2<f32>(1.0), p >= vec2<f32>(0.0));
    }
    return e * 0.5 + 0.5;
}

@fragment
fn fs_main(in: VertexOut) -> FragmentOut {
    let n0 = normalize(in.normal);
    let v = normalize(lighting.camera_pos.xyz - in.world_pos);
    let sun = lighting.sun_dir.xyz;
    let env = 0.5 * (lighting.ambient_sky.rgb + lighting.ambient_ground.rgb) + lighting.sun_color.rgb * 0.5;
    var colour: vec3<f32>;
    if lighting.water_params.y > 0.5 {
        // From below: the lit underside, deeper towards grazing angles.
        let facing = clamp(v.y, 0.0, 1.0);
        let body = mix(lighting.water_deep.rgb, lighting.water_shallow.rgb, facing);
        colour = body * (env + vec3<f32>(0.1));
    } else {
        let n_dot_v = clamp(dot(n0, v), 0.0, 1.0);
        let fresnel = 0.02 + 0.98 * pow(1.0 - n_dot_v, 5.0);
        let body = mix(lighting.water_deep.rgb, lighting.water_shallow.rgb, pow(n_dot_v, 2.0));
        let lit = body * (env + vec3<f32>(0.1));
        let r = reflect(-v, n0);
        let sky = sky_gradient(r);
        let glint = lighting.sun_color.rgb * pow(max(dot(r, sun), 0.0), 220.0) * 2.0;
        colour = mix(lit, sky, fresnel) + glint;
    }
    var out: FragmentOut;
    out.color = vec4<f32>(apply_fog(colour, in.world_pos), 1.0);
    out.motion = 0.5 * (in.clip.xy / in.clip.w - in.prev_clip.xy / in.prev_clip.w);
    out.normal = encode_octahedral(n0);
    return out;
}
