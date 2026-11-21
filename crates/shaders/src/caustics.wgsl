// Caustics: an animated cell-noise pattern of bright lines projected on the
// surfaces below the water, from the sun through the surface.
//
// Each layer is the line pattern `1 - smoothstep(0, W, F2 - F1)` of a Worley
// field whose feature points orbit inside their cells; two layers with
// different scales, drift directions and time offsets are combined with a
// geometric mean. The detail is box-filtered with the pixel footprint
// (`fwidth` of the world position, measured by the caller in uniform control
// flow): lines thinner than a pixel fade to the pattern's mean, so nothing
// shimmers at a distance.

#include "noise.wgsl"
#include "lighting.wgsl"

const CAUSTIC_TAU: f32 = 6.2831853;
// Mean of the line pattern; the value a layer fades to when unresolved.
const CAUSTIC_MEAN: f32 = 0.27;
const CAUSTIC_LINE_WIDTH: f32 = 0.22;

// One layer at cell-space position `p` and time `t`, with `footprint` the
// pixel size in cells.
fn caustic_layer(p: vec2<f32>, t: f32, footprint: f32) -> f32 {
    let base = vec2<i32>(floor(p));
    var f1 = 8.0;
    var f2 = 8.0;
    for (var iy = -1; iy <= 1; iy++) {
        for (var ix = -1; ix <= 1; ix++) {
            let cell = base + vec2<i32>(ix, iy);
            let a = hash2(cell);
            let b = hash2(cell + vec2<i32>(57, 113));
            let orbit = 0.5 + 0.38 * vec2<f32>(sin(t + CAUSTIC_TAU * a), cos(t * 0.87 + CAUSTIC_TAU * b));
            let d = length(vec2<f32>(cell) + orbit - p);
            if d < f1 {
                f2 = f1;
                f1 = d;
            } else if d < f2 {
                f2 = d;
            }
        }
    }
    let line = 1.0 - smoothstep(0.0, CAUSTIC_LINE_WIDTH, f2 - f1);
    let resolved = 1.0 - smoothstep(0.25, 0.6, footprint);
    return mix(CAUSTIC_MEAN, line, resolved);
}

// Caustic pattern value in [0, 1] at a world position, `dpos` being the
// per-pixel change of the world position.
fn caustic_pattern(world_pos: vec3<f32>, dpos: vec3<f32>, t: f32) -> f32 {
    let wide = max(dpos.x, dpos.z);
    let s1 = 0.45;
    let s2 = 0.29;
    let a = caustic_layer(world_pos.xz * s1 + vec2<f32>(0.11, 0.07) * t, t * 0.9, wide * s1);
    let b = caustic_layer(world_pos.xz * s2 - vec2<f32>(0.05, 0.12) * t + vec2<f32>(31.0, 17.0), t * 0.6 + 2.0, wide * s2);
    return sqrt(a * b);
}

// Extra light, to be multiplied by the albedo, that the caustics add on a
// surface at `world_pos` with unit normal `n`: zero above the surface, and
// fading with the depth below it.
fn caustic_light(world_pos: vec3<f32>, n: vec3<f32>, dpos: vec3<f32>, t: f32) -> vec3<f32> {
    let pattern = caustic_pattern(world_pos, dpos, t);
    let strength = lighting.water_params.z;
    let depth = lighting.water_params.x - world_pos.y;
    if strength <= 0.0 || depth <= 0.0 {
        return vec3<f32>(0.0);
    }
    let facing = max(n.y, 0.0);
    let falloff = exp(-0.10 * depth);
    return lighting.sun_color.rgb * (2.2 * strength * pattern * facing * falloff);
}
