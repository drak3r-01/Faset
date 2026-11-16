// Procedural materials: a pattern chosen by the material id, evaluated in
// world space with triplanar projection.
//
// Every pattern returns a blend factor `t` between colour A and colour B.
// Fine detail is box-filtered with the pixel footprint (from `fwidth`) and
// fades to the pattern's mean once a cell is smaller than a pixel, so
// distant surfaces do not shimmer.

#include "noise.wgsl"

// Pattern ids; must match `render::Pattern`.
const PATTERN_FLAT: u32 = 0u;
const PATTERN_GRADIENT: u32 = 1u;
const PATTERN_NOISE: u32 = 2u;
const PATTERN_CHECKER: u32 = 3u;
const PATTERN_STRIPES: u32 = 4u;
const PATTERN_GRID: u32 = 5u;
const PATTERN_BRICK: u32 = 6u;
const PATTERN_PLANKS: u32 = 7u;
const PATTERN_TILES: u32 = 8u;
const PATTERN_RIPPLES: u32 = 9u;
const PATTERN_WATER: u32 = 10u;
const PATTERN_LAVA: u32 = 11u;
const PATTERN_EMISSIVE: u32 = 12u;
const PATTERN_HOLOGRAM: u32 = 13u;

const TAU: f32 = 6.283185307;

struct Material {
    // Linear RGB, may exceed 1 for emissive surfaces; a unused.
    color_a: vec4<f32>,
    color_b: vec4<f32>,
    pattern: u32,
    // Metres per pattern cell.
    scale: f32,
    // Animation speed (cycles per second for animated patterns).
    speed: f32,
    _pad: f32,
}

struct MaterialTable {
    entries: array<Material, 256>,
}

@group(0) @binding(0) var<uniform> materials: MaterialTable;

// Fades a filtered pattern value to its mean as the footprint (cells per
// pixel) grows past half a cell.
fn settle(t: f32, mean: f32, footprint: f32) -> f32 {
    return mix(t, mean, smoothstep(0.5, 1.0, footprint));
}

// Grid lines of width 0.06 on a 1 x 1 cell.
fn grid_pattern(uv: vec2<f32>, fw: vec2<f32>) -> f32 {
    let width = 0.06;
    let lu = filtered_lines(uv.x, width, fw.x);
    let lv = filtered_lines(uv.y, width, fw.y);
    let mean = 1.0 - (1.0 - width) * (1.0 - width);
    return settle(1.0 - (1.0 - lu) * (1.0 - lv), mean, max(fw.x, fw.y));
}

// Bricks 1 x 0.5 cells, every other row shifted by half a brick; mortar
// (t = 1) 0.05 cells wide, slight per-brick tone variation.
fn brick_pattern(uv: vec2<f32>, fw: vec2<f32>) -> f32 {
    let mortar = 0.05;
    let row_h = 0.5;
    let y = uv.y / row_h;
    let row = floor(y);
    let x = uv.x + 0.5 * row;
    let lh = filtered_lines(y, mortar / row_h, fw.y / row_h);
    let lv = filtered_lines(x, mortar, fw.x);
    let joint = 1.0 - (1.0 - lh) * (1.0 - lv);
    let tone = 0.25 * hash2(vec2<i32>(i32(floor(x)), i32(row)));
    let t = mix(tone, 1.0, joint);
    let joint_mean = 1.0 - (1.0 - mortar / row_h) * (1.0 - mortar);
    let mean = mix(0.125, 1.0, joint_mean);
    return settle(t, mean, max(fw.x, fw.y / row_h));
}

// Planks 0.25 cells wide and 2 cells long, ends staggered per row; seams
// (t = 1) and a per-plank tone with light grain.
fn planks_pattern(uv: vec2<f32>, fw: vec2<f32>) -> f32 {
    let board_w = 0.25;
    let board_l = 2.0;
    let seam = 0.04;
    let y = uv.y / board_w;
    let row = floor(y);
    let x = (uv.x + board_l * hash2(vec2<i32>(i32(row), 7))) / board_l;
    let ls = filtered_lines(y, seam, fw.y / board_w);
    let le = filtered_lines(x, seam * board_w / board_l, fw.x / board_l);
    let joint = 1.0 - (1.0 - ls) * (1.0 - le);
    let plank = vec2<i32>(i32(floor(x)), i32(row));
    let grain_k = detail_weight(fw.y, board_w / 4.0);
    let grain = grain_k * 0.15 * (value_noise2(vec2<f32>(uv.x * 0.5, uv.y * 24.0)) - 0.5);
    let tone = 0.15 + 0.35 * hash2(plank) + grain;
    let t = mix(tone, 1.0, joint);
    let mean = mix(0.325, 1.0, seam + seam * board_w / board_l);
    return settle(t, mean, max(fw.x / board_l, fw.y / board_w));
}

// Square tiles of one cell, grout (t = 1) 0.04 cells wide, per-tile tone.
fn tiles_pattern(uv: vec2<f32>, fw: vec2<f32>) -> f32 {
    let grout = 0.04;
    let lu = filtered_lines(uv.x, grout, fw.x);
    let lv = filtered_lines(uv.y, grout, fw.y);
    let joint = 1.0 - (1.0 - lu) * (1.0 - lv);
    let tone = 0.2 * hash2(vec2<i32>(floor(uv)));
    let t = mix(tone, 1.0, joint);
    let mean = mix(0.1, 1.0, 1.0 - (1.0 - grout) * (1.0 - grout));
    return settle(t, mean, max(fw.x, fw.y));
}

// Wind ripples: crests one cell apart, bent by low-frequency noise.
fn ripples_pattern(uv: vec2<f32>, fw: vec2<f32>) -> f32 {
    let bend = 0.6 * value_noise2(uv * 0.35);
    let k = detail_weight(max(fw.x, fw.y), 1.0);
    let crest = sin(TAU * (uv.x + bend));
    return 0.5 + 0.5 * k * crest * crest * sign(crest);
}

// Two drifting wave trains and slow noise.
fn water_pattern(uv: vec2<f32>, fw: vec2<f32>, phase: f32) -> f32 {
    let f = max(fw.x, fw.y);
    let a = sin(TAU * (uv.x + 0.3 * value_noise2(uv * 0.5 + phase * 0.1) + phase));
    let b = sin(TAU * (0.7 * uv.y - 0.4 * uv.x - 0.8 * phase));
    let k = detail_weight(f, 0.7);
    return 0.5 + 0.25 * k * (a + b);
}

// Blend factor of a 2D pattern on one triplanar plane.
fn planar_pattern(pattern: u32, uv: vec2<f32>, fw: vec2<f32>, phase: f32) -> f32 {
    switch pattern {
        case PATTERN_CHECKER: {
            let a = filtered_square(uv.x, fw.x);
            let b = filtered_square(uv.y, fw.y);
            let t = a + b - 2.0 * a * b;
            return settle(t, 0.5, max(fw.x, fw.y));
        }
        case PATTERN_STRIPES: {
            // Stripes half a cell wide along u.
            return settle(filtered_square(2.0 * uv.x, 2.0 * fw.x), 0.5, 2.0 * fw.x);
        }
        case PATTERN_GRID: {
            return grid_pattern(uv, fw);
        }
        case PATTERN_BRICK: {
            return brick_pattern(uv, fw);
        }
        case PATTERN_PLANKS: {
            return planks_pattern(uv, fw);
        }
        case PATTERN_TILES: {
            return tiles_pattern(uv, fw);
        }
        case PATTERN_RIPPLES: {
            return ripples_pattern(uv, fw);
        }
        case PATTERN_WATER: {
            return water_pattern(uv, fw, phase);
        }
        default: {
            return 0.0;
        }
    }
}

// Linear colour of a material at a world position.
//
// `normal` is the unit world normal; `dpos` is `fwidth(world_pos)`, taken
// by the caller in uniform control flow; `time` is in seconds.
fn material_color(id: u32, world_pos: vec3<f32>, normal: vec3<f32>, dpos: vec3<f32>, time: f32) -> vec3<f32> {
    let m = materials.entries[min(id, 255u)];
    let scale = max(m.scale, 1.0e-3);
    let p = world_pos / scale;
    let dp = dpos / scale;
    let phase = time * m.speed;
    let a = m.color_a.rgb;
    let b = m.color_b.rgb;

    switch m.pattern {
        case PATTERN_FLAT: {
            return a;
        }
        case PATTERN_GRADIENT: {
            return mix(a, b, clamp(p.y, 0.0, 1.0));
        }
        case PATTERN_NOISE: {
            return mix(a, b, fbm3(p, max(dp.x, max(dp.y, dp.z))));
        }
        case PATTERN_LAVA: {
            let footprint = max(dp.x, max(dp.y, dp.z));
            let n = fbm3(p + vec3<f32>(0.0, -0.5 * phase, 0.3 * phase), footprint);
            let glow = smoothstep(0.45, 0.75, n);
            let pulse = 1.0 + 0.15 * sin(TAU * phase);
            return mix(a, b * pulse, glow);
        }
        case PATTERN_EMISSIVE: {
            return a * (1.0 + 0.25 * sin(TAU * phase));
        }
        case PATTERN_HOLOGRAM: {
            // Scanlines a tenth of a cell apart scrolling up, plus flicker.
            let y = 10.0 * (p.y - phase);
            let lines = settle(filtered_square(y, 10.0 * dp.y), 0.5, 10.0 * dp.y);
            let flicker = 0.9 + 0.1 * value_noise2(vec2<f32>(8.0 * phase, 0.0));
            return mix(a, b, lines) * flicker;
        }
        default: {
            // Triplanar: blend the three axis projections by the normal.
            var w = pow(abs(normal), vec3<f32>(4.0));
            w /= max(w.x + w.y + w.z, 1.0e-6);
            var t = 0.0;
            if w.x > 0.01 {
                t += w.x * planar_pattern(m.pattern, p.zy, dp.zy, phase);
            }
            if w.y > 0.01 {
                t += w.y * planar_pattern(m.pattern, p.xz, dp.xz, phase);
            }
            if w.z > 0.01 {
                t += w.z * planar_pattern(m.pattern, p.xy, dp.xy, phase);
            }
            return mix(a, b, clamp(t, 0.0, 1.0));
        }
    }
}
