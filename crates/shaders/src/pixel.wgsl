// Pixel-art materials (pattern ids 14..=27).
//
// Each pattern is a function of an integer texel coordinate (16 texels per
// material cell, nearest filtering, no interpolation) and returns
// `(t, shade)`: `t` blends colour A to colour B, `shade` multiplies the
// result in a few discrete steps, so a surface reads as hand-placed pixels.
// Everything comes from integer hashes: exact and identical on every GPU.
// Texels smaller than a screen pixel fade to the pattern's mean so distant
// surfaces do not shimmer.

#include "noise.wgsl"

fn pmod(a: i32, b: i32) -> i32 {
    return ((a % b) + b) % b;
}

fn pdiv(a: i32, b: i32) -> i32 {
    return (a - pmod(a, b)) / b;
}

// Hash of a texel with a per-pattern salt, in [0, 1].
fn th(q: vec2<i32>, salt: i32) -> f32 {
    return hash2(q + vec2<i32>(salt * 7919, salt * 104729));
}

// Quantises a level in [0, 1] to `n` equally spaced values in [0, 1].
fn steps(x: f32, n: f32) -> f32 {
    return floor(clamp(x, 0.0, 0.999) * n) / (n - 1.0);
}

// Brick wall: bricks 16 x 8 texels, every other row shifted by half a brick,
// mortar one texel wide (t = 1), lit top edge and shadowed bottom edge.
fn px_brick(q: vec2<i32>) -> vec2<f32> {
    let row = pdiv(q.y, 8);
    let y = pmod(q.y, 8);
    let sx = q.x + 8 * (row & 1);
    let x = pmod(sx, 16);
    if y == 0 || x == 0 {
        return vec2<f32>(1.0, 0.9);
    }
    var shade = 0.86 + 0.14 * steps(th(vec2<i32>(pdiv(sx, 16), row), 1), 3.0);
    let speck = th(q, 2);
    if speck > 0.93 {
        shade -= 0.12;
    } else if speck < 0.06 {
        shade += 0.1;
    }
    if y == 1 {
        shade += 0.1;
    }
    if y == 7 {
        shade -= 0.12;
    }
    return vec2<f32>(0.0, shade);
}

// Wooden boards 4 texels wide, ends staggered per board, dark seams (t = 1),
// grain streaks and the odd knot.
fn px_wood(q: vec2<i32>) -> vec2<f32> {
    let row = pdiv(q.y, 4);
    let y = pmod(q.y, 4);
    let sx = q.x + i32(floor(32.0 * th(vec2<i32>(row, 0), 3)));
    let board = vec2<i32>(pdiv(sx, 32), row);
    if y == 0 || pmod(sx, 32) == 0 {
        return vec2<f32>(1.0, 0.8);
    }
    let tone = steps(th(board, 4), 3.0);
    let streak = th(vec2<i32>(pdiv(sx, 6), q.y), 5);
    var shade = 0.92 + 0.16 * tone - 0.1 * select(0.0, 1.0, streak > 0.8);
    if y == 1 {
        shade += 0.06;
    }
    if th(board, 6) > 0.9 {
        let k = vec2<i32>(pmod(sx, 32) - 16, y - 2);
        if abs(k.x) <= 1 && abs(k.y) <= 1 {
            shade -= 0.25;
        }
    }
    return vec2<f32>(0.0, shade);
}

// Grass: three tones of speckle with taller blades (t = 1) in clumps.
fn px_grass(q: vec2<i32>) -> vec2<f32> {
    let clump = th(vec2<i32>(pdiv(q.x, 3), pdiv(q.y, 3)), 7);
    let blade = th(q, 8);
    var t = 0.0;
    if blade > 0.78 {
        t = 1.0;
    } else if clump > 0.6 {
        t = 0.45;
    }
    let shade = 0.88 + 0.08 * steps(th(q, 9), 3.0) + 0.06 * steps(clump, 2.0);
    return vec2<f32>(t, shade);
}

// Sand: fine grain, a few dark pebbles (t = 1) and diagonal wind ripples.
fn px_sand(q: vec2<i32>) -> vec2<f32> {
    var t = 0.0;
    if th(q, 10) > 0.92 {
        t = 1.0;
    }
    let offset = i32(floor(3.0 * th(vec2<i32>(pdiv(q.y, 8), 0), 11)));
    var shade = 0.94 + 0.08 * steps(th(q, 12), 3.0);
    if pmod(q.x + 2 * q.y + offset, 11) == 0 {
        shade -= 0.1;
    }
    return vec2<f32>(t, shade);
}

// Cobblestones: rows 6 texels high, stones of varying width, dark joints
// (t = 1), a lit upper edge.
fn px_stone(q: vec2<i32>) -> vec2<f32> {
    let row = pdiv(q.y, 6);
    let y = pmod(q.y, 6);
    let width = 7 + i32(floor(6.0 * th(vec2<i32>(row, 0), 13)));
    let sx = q.x + i32(floor(16.0 * th(vec2<i32>(row, 1), 14)));
    let col = pdiv(sx, width);
    let x = pmod(sx, width);
    if y == 0 || x == 0 {
        return vec2<f32>(1.0, 0.85);
    }
    let stone = vec2<i32>(col, row);
    var shade = 0.85 + 0.2 * steps(th(stone, 15), 4.0);
    if th(q, 16) > 0.9 {
        shade -= 0.1;
    }
    if y == 1 {
        shade += 0.08;
    }
    return vec2<f32>(steps(th(stone, 17), 3.0) * 0.35, shade);
}

// Concrete: fine noise, pits (t = 1) and panel seams every 16 texels.
fn px_concrete(q: vec2<i32>) -> vec2<f32> {
    var shade = 0.93 + 0.1 * steps(th(q, 18), 3.0);
    var t = 0.0;
    if th(q, 19) > 0.965 {
        t = 1.0;
        shade -= 0.1;
    }
    let x = pmod(q.x, 16);
    let y = pmod(q.y, 16);
    if x == 0 || y == 0 {
        shade -= 0.14;
    } else if x == 1 || y == 1 {
        shade += 0.05;
    }
    return vec2<f32>(t, shade);
}

// Metal plates 16 x 16 texels: lit bevel, rivets (t = 1), brushed streaks.
fn px_metal(q: vec2<i32>) -> vec2<f32> {
    let x = pmod(q.x, 16);
    let y = pmod(q.y, 16);
    var shade = 0.96 + 0.05 * steps(th(vec2<i32>(0, q.y), 20), 3.0);
    var t = 0.0;
    if x == 0 || y == 0 {
        shade = 0.7;
    } else if x == 1 || y == 1 {
        shade = 1.12;
    } else if x == 15 || y == 15 {
        shade = 0.84;
    }
    if (x == 3 || x == 12) && (y == 3 || y == 12) {
        t = 1.0;
        shade = 1.1;
    }
    return vec2<f32>(t, shade);
}

// Water: drifting bands of three tones with white sparkles (t = 1).
fn px_water(q: vec2<i32>, phase: f32) -> vec2<f32> {
    let drift = i32(floor(phase * 8.0));
    var t = 0.0;
    if pmod(q.x + q.y * 2 - drift, 16) < 3 {
        t = 0.55;
    }
    if th(vec2<i32>(q.x + drift / 2, q.y), 21) > 0.985 {
        t = 1.0;
    }
    let cell = vec2<i32>(pdiv(q.x + drift, 4), pdiv(q.y, 4));
    return vec2<f32>(t, 0.92 + 0.1 * steps(th(cell, 22), 3.0));
}

// Tiles 8 x 8 texels in a checker of two tones, grout (t = 1).
fn px_tile(q: vec2<i32>) -> vec2<f32> {
    let x = pmod(q.x, 8);
    let y = pmod(q.y, 8);
    if x == 0 || y == 0 {
        return vec2<f32>(1.0, 0.85);
    }
    let t = f32(pmod(pdiv(q.x, 8) + pdiv(q.y, 8), 2)) * 0.6;
    var shade = 0.96 + 0.06 * steps(th(q, 23), 2.0);
    if x == 1 || y == 1 {
        shade += 0.07;
    }
    return vec2<f32>(t, shade);
}

// Packed earth: dark speckle, light pebbles (t = 1).
fn px_dirt(q: vec2<i32>) -> vec2<f32> {
    let g = th(q, 24);
    var t = 0.0;
    var shade = 0.82 + 0.16 * steps(th(vec2<i32>(pdiv(q.x, 2), pdiv(q.y, 2)), 25), 3.0);
    if g > 0.95 {
        t = 1.0;
        shade = 1.0;
    } else if g < 0.1 {
        shade -= 0.12;
    }
    return vec2<f32>(t, shade);
}

// Snow: near white with cool shadow flecks (t = 1) and sparkles.
fn px_snow(q: vec2<i32>) -> vec2<f32> {
    let g = th(q, 26);
    var t = 0.0;
    if g > 0.88 {
        t = 1.0;
    }
    var shade = 0.97 + 0.05 * steps(th(vec2<i32>(pdiv(q.x, 2), pdiv(q.y, 2)), 27), 3.0);
    if g < 0.02 {
        shade = 1.2;
    }
    return vec2<f32>(t, shade);
}

// Crate: a frame 3 texels wide with a diagonal brace around boards (t = .5).
fn px_crate(q: vec2<i32>) -> vec2<f32> {
    let x = pmod(q.x, 16);
    let y = pmod(q.y, 16);
    let frame = x < 3 || x > 12 || y < 3 || y > 12;
    let brace = abs(x - y) < 2 || abs(x + y - 15) < 2;
    if frame || brace {
        var shade = 1.0;
        if x == 0 || y == 0 || x == 15 || y == 15 {
            shade = 0.72;
        }
        return vec2<f32>(0.0, shade);
    }
    let plank = pdiv(y - 3, 3);
    return vec2<f32>(0.5, 0.82 + 0.12 * steps(th(vec2<i32>(plank, 0), 28), 3.0));
}

// Hazard stripes: diagonal bands 8 texels wide (t = 1 for the dark one).
fn px_hazard(q: vec2<i32>) -> vec2<f32> {
    var t = 0.0;
    if pmod(q.x + q.y, 16) >= 8 {
        t = 1.0;
    }
    var shade = 0.96 + 0.06 * steps(th(q, 29), 2.0);
    if pmod(q.x, 16) == 0 || pmod(q.y, 16) == 0 {
        shade -= 0.12;
    }
    return vec2<f32>(t, shade);
}

// Foliage: clumps of leaves in tones, bright tips and dark gaps (t = 1).
fn px_leaves(q: vec2<i32>) -> vec2<f32> {
    let clump = th(vec2<i32>(pdiv(q.x, 4), pdiv(q.y, 4)), 30);
    let g = th(q, 31);
    var t = 0.25 * steps(clump, 3.0);
    var shade = 0.9 + 0.12 * steps(g, 3.0);
    if g > 0.9 {
        t = 0.0;
        shade = 1.2;
    } else if g < 0.1 {
        t = 1.0;
        shade = 0.7;
    }
    return vec2<f32>(t, shade);
}

fn pixel_pattern(pattern: u32, q: vec2<i32>, phase: f32) -> vec2<f32> {
    switch pattern {
        case 14u: { return px_brick(q); }
        case 15u: { return px_wood(q); }
        case 16u: { return px_grass(q); }
        case 17u: { return px_sand(q); }
        case 18u: { return px_stone(q); }
        case 19u: { return px_concrete(q); }
        case 20u: { return px_metal(q); }
        case 21u: { return px_water(q, phase); }
        case 22u: { return px_tile(q); }
        case 23u: { return px_dirt(q); }
        case 24u: { return px_snow(q); }
        case 25u: { return px_crate(q); }
        case 26u: { return px_hazard(q); }
        default: { return px_leaves(q); }
    }
}

// Mean `(t, shade)` of a pixel pattern, used when texels are sub-pixel.
fn pixel_mean(pattern: u32) -> vec2<f32> {
    switch pattern {
        case 14u: { return vec2<f32>(0.2, 0.92); }
        case 15u: { return vec2<f32>(0.1, 0.96); }
        case 16u: { return vec2<f32>(0.3, 0.95); }
        case 17u: { return vec2<f32>(0.08, 0.98); }
        case 18u: { return vec2<f32>(0.2, 0.95); }
        case 19u: { return vec2<f32>(0.04, 0.97); }
        case 20u: { return vec2<f32>(0.01, 0.96); }
        case 21u: { return vec2<f32>(0.25, 0.97); }
        case 22u: { return vec2<f32>(0.4, 0.97); }
        case 23u: { return vec2<f32>(0.06, 0.88); }
        case 24u: { return vec2<f32>(0.12, 0.99); }
        case 25u: { return vec2<f32>(0.3, 0.9); }
        case 26u: { return vec2<f32>(0.5, 0.97); }
        default: { return vec2<f32>(0.3, 0.92); }
    }
}

// One triplanar plane of a pixel pattern at `uv` (in cells).
fn pixel_plane(pattern: u32, uv: vec2<f32>, fw: vec2<f32>, phase: f32) -> vec2<f32> {
    let q = vec2<i32>(floor(uv * 16.0));
    let value = pixel_pattern(pattern, q, phase);
    let footprint = 16.0 * max(fw.x, fw.y);
    return mix(value, pixel_mean(pattern), smoothstep(0.7, 1.6, footprint));
}

// Pixel pattern at a world position: the dominant axis plane wins (powers
// of the normal keep the seams between planes thin and hard).
fn pixel_surface(pattern: u32, p: vec3<f32>, normal: vec3<f32>, dp: vec3<f32>, phase: f32) -> vec2<f32> {
    var w = pow(abs(normal), vec3<f32>(8.0));
    let keep = vec3<f32>(f32(w.x > 0.02 * (w.x + w.y + w.z)), f32(w.y > 0.02 * (w.x + w.y + w.z)), f32(w.z > 0.02 * (w.x + w.y + w.z)));
    w = w * keep;
    w /= max(w.x + w.y + w.z, 1.0e-6);
    var v = vec2<f32>(0.0);
    if w.x > 0.0 {
        v += w.x * pixel_plane(pattern, p.zy, dp.zy, phase);
    }
    if w.y > 0.0 {
        v += w.y * pixel_plane(pattern, p.xz, dp.xz, phase);
    }
    if w.z > 0.0 {
        v += w.z * pixel_plane(pattern, p.xy, dp.xy, phase);
    }
    return v;
}
