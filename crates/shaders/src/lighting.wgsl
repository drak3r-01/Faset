// Stylised lighting: banded diffuse ramp per light, hemispheric ambient,
// rim light and exponential height fog. Mirrors `render::lighting`.
//
//   R_i = mix(shadow * C_i, C_i, smoothstep(t - s, t + s, N.L_i))
//   A   = mix(ground, sky, 0.5 N.y + 0.5)
//   rim = k_r (1 - max(N.V, 0))^p
//   C   = albedo (A + sum_i R_i o_i) + rim + E
//
// o_i is the shadow visibility of light i (1 until lot 2.5).

const MAX_POINT_LIGHTS: u32 = 32u;

struct PointLight {
    // xyz position, w radius.
    position_radius: vec4<f32>,
    // rgb colour, w unused.
    color: vec4<f32>,
}

// Must match `render::lighting::GpuLighting`.
struct Lighting {
    camera_pos: vec4<f32>,
    // Unit vector towards the sun.
    sun_dir: vec4<f32>,
    // Colour times intensity.
    sun_color: vec4<f32>,
    ambient_ground: vec4<f32>,
    ambient_sky: vec4<f32>,
    // Shadow colour as a multiple of the light colour.
    shadow: vec4<f32>,
    // x threshold, y softness, z rim power.
    ramp_rim: vec4<f32>,
    rim_color: vec4<f32>,
    // rgb colour, w density.
    fog_color: vec4<f32>,
    // x falloff, y reference height.
    fog_params: vec4<f32>,
    // x point light count.
    counts: vec4<u32>,
    lights: array<PointLight, 32>,
}

@group(1) @binding(0) var<uniform> lighting: Lighting;

// Banded diffuse response of one light of colour `c` (shadow term o = 1).
fn ramp_light(c: vec3<f32>, n_dot_l: f32) -> vec3<f32> {
    let t = lighting.ramp_rim.x;
    let s = max(lighting.ramp_rim.y, 1.0e-4);
    let band = smoothstep(t - s, t + s, n_dot_l);
    return mix(lighting.shadow.rgb * c, c, band);
}

// Attenuation (1 - (d/r)^2)^2; callers reject d >= r first.
fn point_attenuation(d: f32, r: f32) -> f32 {
    let x = d / r;
    let k = 1.0 - x * x;
    return k * k;
}

// Lit colour before fog. `n` is the unit world normal.
fn shade(albedo: vec3<f32>, emissive: vec3<f32>, n: vec3<f32>, world_pos: vec3<f32>) -> vec3<f32> {
    let ambient = mix(lighting.ambient_ground.rgb, lighting.ambient_sky.rgb, 0.5 * n.y + 0.5);
    var direct = ramp_light(lighting.sun_color.rgb, dot(n, lighting.sun_dir.xyz));
    let count = min(lighting.counts.x, MAX_POINT_LIGHTS);
    for (var i = 0u; i < count; i++) {
        let light = lighting.lights[i];
        let to_light = light.position_radius.xyz - world_pos;
        let d = length(to_light);
        let r = light.position_radius.w;
        if !(d < r) {
            continue;
        }
        let l = to_light / max(d, 1.0e-6);
        direct += ramp_light(light.color.rgb * point_attenuation(d, r), dot(n, l));
    }
    let v = normalize(lighting.camera_pos.xyz - world_pos);
    let rim = lighting.rim_color.rgb * pow(max(1.0 - max(dot(n, v), 0.0), 0.0), lighting.ramp_rim.z);
    return albedo * (ambient + direct) + rim + emissive;
}

// Exponential height fog factor in [0, 1] for a fragment at `world_pos`.
fn fog_factor(world_pos: vec3<f32>) -> f32 {
    let density = lighting.fog_color.w;
    if density <= 0.0 {
        return 0.0;
    }
    let k = lighting.fog_params.x;
    let h0 = lighting.fog_params.y;
    let ray = world_pos - lighting.camera_pos.xyz;
    let d = length(ray);
    let ry = ray.y / max(d, 1.0e-6);
    let x = k * d * ry;
    var path: f32;
    if abs(x) < 1.0e-2 {
        // Series of d (1 - e^-x) / x: no cancellation, covers k = 0.
        path = d * (1.0 - x * (0.5 - x / 6.0));
    } else {
        path = (1.0 - exp(-x)) / (k * ry);
    }
    let y_cam = lighting.camera_pos.y;
    return 1.0 - exp(-density * exp(-k * (y_cam - h0)) * path);
}

// Applies fog to a lit colour.
fn apply_fog(color: vec3<f32>, world_pos: vec3<f32>) -> vec3<f32> {
    return mix(color, lighting.fog_color.rgb, clamp(fog_factor(world_pos), 0.0, 1.0));
}
