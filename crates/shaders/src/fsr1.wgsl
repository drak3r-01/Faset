// FidelityFX Super Resolution 1: a WGSL port of the two spatial passes,
// EASU (edge-adaptive spatial upsampling) and RCAS (robust contrast-adaptive
// sharpening). Mirrors `faset-upscale`.
//
//   fs_easu   input resolution -> display resolution, 12-tap kernel that
//             follows the local edge direction
//   fs_rcas   sharpening at display resolution
//
// Differences from AMD's reference (MIT licensed, ffx_fsr1.h): the taps are
// `textureLoad`s instead of gathers, and the HDR scene colour is compressed
// to [0, 1) (`c / (1 + max(c))`) around the two passes, because both expect
// display-range input. EASU writes compressed values; RCAS expands them.

#include "upscale_common.wgsl"

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;

// ---------------------------------------------------------------------- EASU

// p0 = (input/output scale xy, 0.5 * scale - 0.5 xy)

fn tap(base: vec2<i32>, x: i32, y: i32) -> vec3<f32> {
    return compress(load_clamped(src, base + vec2<i32>(x, y)).rgb);
}

// Luma times two in two multiply-adds.
fn easu_luma(c: vec3<f32>) -> f32 {
    return c.b * 0.5 + (c.r * 0.5 + c.g);
}

fn rcp_lo(x: f32) -> f32 {
    return 1.0 / max(x, 1.0e-9);
}

// One kernel tap: weight from the rotated, anisotropic distance (a
// Lanczos-2 approximation without sin or sqrt). Returns (weighted colour, weight).
fn easu_tap(
    off: vec2<f32>,
    dir: vec2<f32>,
    len: vec2<f32>,
    lob: f32,
    clp: f32,
    c: vec3<f32>,
) -> vec4<f32> {
    var v = vec2<f32>(
        off.x * dir.x + off.y * dir.y,
        off.x * (-dir.y) + off.y * dir.x,
    );
    v = v * len;
    var d2 = v.x * v.x + v.y * v.y;
    // Limit to the window: at a corner two taps can be far outside.
    d2 = min(d2, clp);
    var wb = (2.0 / 5.0) * d2 - 1.0;
    var wa = lob * d2 - 1.0;
    wb = wb * wb;
    wa = wa * wa;
    wb = (25.0 / 16.0) * wb - (25.0 / 16.0 - 1.0);
    let w = wb * wa;
    return vec4<f32>(c * w, w);
}

// Edge direction and length contribution of one of the four bilinear
// quadrants, around the centre tap `lc` with neighbours a (up), b (left),
// d (right) and e (down). Returns (dir.x, dir.y, len).
fn easu_set(w: f32, la: f32, lb: f32, lc: f32, ld: f32, le: f32) -> vec3<f32> {
    var len_x = max(abs(ld - lc), abs(lc - lb));
    let dir_x = ld - lb;
    len_x = clamp(abs(dir_x) * rcp_lo(len_x), 0.0, 1.0);
    len_x = len_x * len_x;
    var len_y = max(abs(le - lc), abs(lc - la));
    let dir_y = le - la;
    len_y = clamp(abs(dir_y) * rcp_lo(len_y), 0.0, 1.0);
    len_y = len_y * len_y;
    return vec3<f32>(dir_x * w, dir_y * w, (len_x + len_y) * w);
}

@fragment
fn fs_easu(in: VertexOut) -> @location(0) vec4<f32> {
    // Position of the sample in input texels, minus one half: `f` is the
    // texel at its top left.
    var pp = floor(in.position.xy) * pc.p0.xy + pc.p0.zw;
    let fp = floor(pp);
    pp = pp - fp;
    let base = vec2<i32>(fp);

    // 12-tap kernel.
    //    b c
    //  e f g h
    //  i j k l
    //    n o
    let b = tap(base, 0, -1);
    let c = tap(base, 1, -1);
    let e = tap(base, -1, 0);
    let f = tap(base, 0, 0);
    let g = tap(base, 1, 0);
    let h = tap(base, 2, 0);
    let i = tap(base, -1, 1);
    let j = tap(base, 0, 1);
    let k = tap(base, 1, 1);
    let l = tap(base, 2, 1);
    let n = tap(base, 0, 2);
    let o = tap(base, 1, 2);

    let b_l = easu_luma(b);
    let c_l = easu_luma(c);
    let e_l = easu_luma(e);
    let f_l = easu_luma(f);
    let g_l = easu_luma(g);
    let h_l = easu_luma(h);
    let i_l = easu_luma(i);
    let j_l = easu_luma(j);
    let k_l = easu_luma(k);
    let l_l = easu_luma(l);
    let n_l = easu_luma(n);
    let o_l = easu_luma(o);

    // Accumulate the gradient for bilinear interpolation.
    var acc = easu_set((1.0 - pp.x) * (1.0 - pp.y), b_l, e_l, f_l, g_l, j_l);
    acc += easu_set(pp.x * (1.0 - pp.y), c_l, f_l, g_l, h_l, k_l);
    acc += easu_set((1.0 - pp.x) * pp.y, f_l, i_l, j_l, k_l, n_l);
    acc += easu_set(pp.x * pp.y, g_l, j_l, k_l, l_l, o_l);
    var dir = acc.xy;
    var len = acc.z;

    // Normalise, and clean up close to zero.
    var dir_r = dir.x * dir.x + dir.y * dir.y;
    let zro = dir_r < 1.0 / 32768.0;
    dir_r = select(inverseSqrt(dir_r), 1.0, zro);
    dir.x = select(dir.x, 1.0, zro);
    dir = dir * dir_r;

    // From {0 to 2} to {0 to 1}, shaped with a square.
    len = len * 0.5;
    len = len * len;
    // Stretch the kernel: 1 along an axis, sqrt(2) on a diagonal.
    let stretch = (dir.x * dir.x + dir.y * dir.y) * rcp_lo(max(abs(dir.x), abs(dir.y)));
    // Anisotropic length after rotation: x from 1 to `stretch` on edges, y
    // from 1 to 2.
    let len2 = vec2<f32>(1.0 + (stretch - 1.0) * len, 1.0 - 0.5 * len);
    // The window moves from +-sqrt(2) to slightly beyond 2 with the edge.
    let lob = 0.5 + ((1.0 / 4.0 - 0.04) - 0.5) * len;
    let clp = rcp_lo(lob);

    // Min and max of the four nearest taps, to dering the result.
    let min4 = min(min(f, g), min(j, k));
    let max4 = max(max(f, g), max(j, k));

    var sum = easu_tap(vec2<f32>(0.0, -1.0) - pp, dir, len2, lob, clp, b);
    sum += easu_tap(vec2<f32>(1.0, -1.0) - pp, dir, len2, lob, clp, c);
    sum += easu_tap(vec2<f32>(-1.0, 1.0) - pp, dir, len2, lob, clp, i);
    sum += easu_tap(vec2<f32>(0.0, 1.0) - pp, dir, len2, lob, clp, j);
    sum += easu_tap(vec2<f32>(0.0, 0.0) - pp, dir, len2, lob, clp, f);
    sum += easu_tap(vec2<f32>(-1.0, 0.0) - pp, dir, len2, lob, clp, e);
    sum += easu_tap(vec2<f32>(1.0, 1.0) - pp, dir, len2, lob, clp, k);
    sum += easu_tap(vec2<f32>(2.0, 1.0) - pp, dir, len2, lob, clp, l);
    sum += easu_tap(vec2<f32>(2.0, 0.0) - pp, dir, len2, lob, clp, h);
    sum += easu_tap(vec2<f32>(1.0, 0.0) - pp, dir, len2, lob, clp, g);
    sum += easu_tap(vec2<f32>(1.0, 2.0) - pp, dir, len2, lob, clp, o);
    sum += easu_tap(vec2<f32>(0.0, 2.0) - pp, dir, len2, lob, clp, n);

    let pix = min(max4, max(min4, sum.rgb / max(sum.w, 1.0e-6)));
    return vec4<f32>(pix, 1.0);
}

// ---------------------------------------------------------------------- RCAS

// p0.x = exp2(-sharpness), sharpness in stops (0 is the sharpest).
const RCAS_LIMIT: f32 = 0.1875;

@fragment
fn fs_rcas(in: VertexOut) -> @location(0) vec4<f32> {
    let p = vec2<i32>(in.position.xy);
    // The minimal 3x3 neighbourhood: a cross.
    //    b
    //  d e f
    //    h
    let b = load_clamped(src, p + vec2<i32>(0, -1)).rgb;
    let d = load_clamped(src, p + vec2<i32>(-1, 0)).rgb;
    let e = load_clamped(src, p).rgb;
    let f = load_clamped(src, p + vec2<i32>(1, 0)).rgb;
    let h = load_clamped(src, p + vec2<i32>(0, 1)).rgb;

    // Min and max of the ring.
    let mn4 = min(min(b, d), min(f, h));
    let mx4 = max(max(b, d), max(f, h));
    // Limiters: how far the lobe can go before the result clips.
    // (The guards keep black and saturated rings finite.)
    let hit_min = min(mn4, e) / max(4.0 * mx4, vec3<f32>(1.0e-6));
    let hit_max = (1.0 - max(mx4, e)) / min(4.0 * mn4 - 4.0, vec3<f32>(-1.0e-6));
    let lobe_c = max(-hit_min, hit_max);
    let lobe = max(-RCAS_LIMIT, min(max(lobe_c.r, max(lobe_c.g, lobe_c.b)), 0.0)) * pc.p0.x;
    let rcp_l = 1.0 / (4.0 * lobe + 1.0);
    let pix = (lobe * b + lobe * d + lobe * h + lobe * f + e) * rcp_l;
    return vec4<f32>(decompress(pix), 1.0);
}
