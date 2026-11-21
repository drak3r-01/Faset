// Stylised lighting: banded diffuse ramp per light, hemispheric ambient,
// rim light, exponential height fog and cascaded sun shadows. Mirrors
// `render::lighting` and `render::shadow`.
//
//   R_i = mix(shadow * C_i, C_i, o_i smoothstep(t - s, t + s, N.L_i))
//   A   = mix(ground, sky, 0.5 N.y + 0.5)
//   rim = k_r (1 - max(N.V, 0))^p
//   C   = albedo (A + sum_i R_i) + rim + E
//
// o_i is the shadow visibility of light i, in [0, 1]: 1 for the point lights
// (they cast no shadows) and the 3 x 3 PCF cascade lookup for the sun.
// Decision: o scales the ramp band instead of the whole of R_i, so a cast
// shadow takes exactly the tone of the terminator side (shadow * C_i) and
// back faces, where the lookup is meaningless, are never affected.

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
    // Clip-from-world of the three sun shadow cascades.
    shadow_matrices: array<mat4x4<f32>, 3>,
    // xyz far view depth of each cascade, w 1 when the sun casts shadows.
    shadow_splits: vec4<f32>,
    // xyz world size of one shadow texel of each cascade.
    shadow_texel: vec4<f32>,
    // x depth bias, y slope bias, z normal offset (all in texels).
    shadow_bias: vec4<f32>,
}

const SHADOW_MAP_SIZE: f32 = 2048.0;

@group(1) @binding(0) var<uniform> lighting: Lighting;
@group(1) @binding(1) var shadow_map: texture_depth_2d_array;
@group(1) @binding(2) var shadow_sampler: sampler_comparison;

// Banded diffuse response of one light of colour `c` with shadow
// visibility `o`.
fn ramp_light(c: vec3<f32>, n_dot_l: f32, o: f32) -> vec3<f32> {
    let t = lighting.ramp_rim.x;
    let s = max(lighting.ramp_rim.y, 1.0e-4);
    let band = smoothstep(t - s, t + s, n_dot_l);
    return mix(lighting.shadow.rgb * c, c, band * o);
}

// Sun shadow visibility in [0, 1] of a fragment at `world_pos` with unit
// normal `n` and view depth `view_depth` (distance along the view axis):
// picks the cascade, biases the receiver and averages 3 x 3 nearest
// comparisons (percentage-closer filtering).
fn sun_shadow(world_pos: vec3<f32>, n: vec3<f32>, view_depth: f32) -> f32 {
    if lighting.shadow_splits.w < 0.5 || view_depth > lighting.shadow_splits.z {
        return 1.0;
    }
    var cascade = 0u;
    if view_depth > lighting.shadow_splits.y {
        cascade = 2u;
    } else if view_depth > lighting.shadow_splits.x {
        cascade = 1u;
    }
    let texel = lighting.shadow_texel[cascade];
    let sun = lighting.sun_dir.xyz;
    let cos_t = clamp(dot(n, sun), 0.0, 1.0);
    let sin_t = sqrt(1.0 - cos_t * cos_t);
    let tan_t = min(sin_t / max(cos_t, 1.0e-3), 8.0);
    // Receiver pushed towards the sun (depth bias) and off the surface.
    let bias = lighting.shadow_bias;
    let p = world_pos + n * (bias.z * sin_t * texel) + sun * ((bias.x + bias.y * tan_t) * texel);
    let ndc = lighting.shadow_matrices[cascade] * vec4<f32>(p, 1.0);
    let uv = ndc.xy * 0.5 + vec2<f32>(0.5);
    var sum = 0.0;
    for (var iy = -1; iy <= 1; iy++) {
        for (var ix = -1; ix <= 1; ix++) {
            let offset = vec2<f32>(f32(ix), f32(iy)) / SHADOW_MAP_SIZE;
            sum += textureSampleCompareLevel(shadow_map, shadow_sampler, uv + offset, cascade, ndc.z);
        }
    }
    return sum / 9.0;
}

// Attenuation (1 - (d/r)^2)^2; callers reject d >= r first.
fn point_attenuation(d: f32, r: f32) -> f32 {
    let x = d / r;
    let k = 1.0 - x * x;
    return k * k;
}

// Lit colour before fog. `n` is the unit world normal, `view_depth` the
// distance of the fragment along the view axis (clip w).
fn shade(
    albedo: vec3<f32>,
    emissive: vec3<f32>,
    n: vec3<f32>,
    world_pos: vec3<f32>,
    view_depth: f32,
) -> vec3<f32> {
    let ambient = mix(lighting.ambient_ground.rgb, lighting.ambient_sky.rgb, 0.5 * n.y + 0.5);
    let sun_n_dot_l = dot(n, lighting.sun_dir.xyz);
    var direct = ramp_light(lighting.sun_color.rgb, sun_n_dot_l, sun_shadow(world_pos, n, view_depth));
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
        direct += ramp_light(light.color.rgb * point_attenuation(d, r), dot(n, l), 1.0);
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
