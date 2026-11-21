// Wet surfaces (humidity): damp surfaces are darker, up-facing ones collect
// puddles that reflect the sky and show raindrop ripples. Used by the opaque
// pass; with a humidity of 0 the surface is returned untouched.
//
//   albedo' = albedo (1 - 0.4 H)                       (damp)
//   puddle  = smoothstep(t, t + 0.08, noise(xz)) up(N)  (t falls with H)
//   C       = mix(C_lit, C_sky(reflect(-V, N')), w)     (w from puddle, Fresnel)
//
// The ripples are rings that expand from random drop points on a grid of
// 0.6 m cells, one drop per cell at a time (this lot's own pattern; the
// wind-ripple pattern of lot 2.3 is for sand dunes).

#include "noise.wgsl"
#include "lighting.wgsl"

const RIPPLE_CELL: f32 = 0.6;
const RIPPLE_MAX_RADIUS: f32 = 0.28;
const RIPPLE_WIDTH: f32 = 0.025;

// Raindrop ripples at the ground position `xz`: the surface tilt (xy) and the
// ring energy (z), all scaled by `strength`.
fn raindrop_ripples(xz: vec2<f32>, time: f32, strength: f32) -> vec3<f32> {
    let base = vec2<i32>(floor(xz / RIPPLE_CELL));
    var tilt = vec2<f32>(0.0);
    var energy = 0.0;
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            let c = base + vec2<i32>(dx, dy);
            let center = (vec2<f32>(c) + vec2<f32>(
                hash2(c + vec2<i32>(17, 3)),
                hash2(c + vec2<i32>(5, 91)),
            )) * RIPPLE_CELL;
            let period = 0.9 + 0.8 * hash2(c + vec2<i32>(3, 7));
            let age = fract(time / period + hash2(c + vec2<i32>(41, 13)));
            let d = distance(xz, center);
            let x = (d - age * RIPPLE_MAX_RADIUS) / RIPPLE_WIDTH;
            let ring = exp(-x * x) * (1.0 - age) * (1.0 - age);
            let dir = (xz - center) / max(d, 1.0e-4);
            tilt += dir * ring * sign(x);
            energy += ring;
        }
    }
    return vec3<f32>(tilt * (0.9 * strength), energy * strength);
}

struct WetSurface {
    // Albedo after damp darkening.
    albedo: vec3<f32>,
    // Reflected light and how much of it replaces the lit colour.
    reflection: vec3<f32>,
    weight: f32,
}

fn wet_surface(albedo: vec3<f32>, n: vec3<f32>, world_pos: vec3<f32>, time: f32) -> WetSurface {
    var out: WetSurface;
    out.albedo = albedo;
    out.reflection = vec3<f32>(0.0);
    out.weight = 0.0;
    let humidity = lighting.weather_wind.w;
    if humidity <= 0.0 {
        return out;
    }
    let rain = lighting.weather_precip.x;
    let up = smoothstep(0.65, 0.95, n.y);

    // Damp everywhere, darker in puddles.
    let noise = 0.6 * value_noise2(world_pos.xz * 0.35) + 0.4 * value_noise2(world_pos.xz * 1.3 + vec2<f32>(7.0, 3.0));
    let threshold = mix(0.95, 0.40, humidity);
    let puddle = smoothstep(threshold, threshold + 0.08, noise) * up;
    out.albedo = albedo * (1.0 - 0.4 * humidity) * mix(1.0, 0.5, puddle);

    // Reflection direction, tilted by the ripples while it rains.
    let ripples = raindrop_ripples(world_pos.xz, time, puddle * smoothstep(0.0, 0.25, rain));
    let nr = normalize(n + vec3<f32>(ripples.x, 0.0, ripples.y));
    let v = normalize(lighting.camera_pos.xyz - world_pos);
    let r = reflect(-v, nr);
    let fresnel = 0.04 + 0.96 * pow(1.0 - max(dot(nr, v), 0.0), 5.0);
    let sky = sky_gradient(vec3<f32>(r.x, max(r.y, 0.02), r.z));
    let glint = lighting.sun_color.rgb * (6.0 * pow(max(dot(r, lighting.sun_dir.xyz), 0.0), 120.0));
    let env = 0.5 * (lighting.ambient_sky.rgb + lighting.ambient_ground.rgb);
    out.reflection = sky + glint + env * (0.6 * ripples.z);
    // Puddles are glossy; merely damp up-facing surfaces have a faint sheen.
    let gloss = max(puddle, 0.3 * humidity * up);
    out.weight = clamp(gloss * (0.35 + 0.65 * fresnel), 0.0, 0.9);
    return out;
}
