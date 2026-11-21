// Post-processing chain on the HDR colour buffer. Mirrors `render::post`.
//
// Every pass draws one full-screen triangle and reads up to three images
// bound at set 0 (`src`, `aux`, `extra`); the pass parameters travel in the
// push constant block (eight vec4). Entry points, one per pass:
//
//   fs_contours  Sobel edges on linear depth and normals (src = HDR,
//                aux = depth, extra = normals)
//   fs_down      bloom downsample; a soft threshold is applied when p0.z > 0
//   fs_up        bloom upsample (blended additively into the larger level)
//   fs_tonemap   bloom add, exposure, ACES, clamp, grading (src = colour,
//                aux = bloom)
//   fs_effects   underwater ripple, chromatic aberration, vignette, grain,
//                damage flash and speed lines
//   fs_fxaa      FXAA on the finished image

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;
@group(0) @binding(2) var aux: texture_2d<f32>;
@group(0) @binding(3) var extra: texture_2d<f32>;

struct Push {
    p0: vec4<f32>,
    p1: vec4<f32>,
    p2: vec4<f32>,
    p3: vec4<f32>,
    p4: vec4<f32>,
    p5: vec4<f32>,
    p6: vec4<f32>,
    p7: vec4<f32>,
}

var<immediate> pc: Push;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32) -> VertexOut {
    let uv = vec2<f32>(f32((vertex << 1u) & 2u), f32(vertex & 2u));
    var out: VertexOut;
    out.position = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    out.uv = uv;
    return out;
}

fn fetch(tex: texture_2d<f32>, p: vec2<i32>) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(tex));
    return textureLoad(tex, clamp(p, vec2<i32>(0), size - vec2<i32>(1)), 0);
}

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

fn hash12(p: vec2<f32>) -> f32 {
    var q = fract(p * vec2<f32>(0.1031, 0.1030));
    q += dot(q, q.yx + 33.33);
    return fract((q.x + q.y) * q.x);
}

// ---------------------------------------------------------------- contours

// p0 = (thickness px, strength, depth threshold, normal threshold)
// p1 = (colour rgb, near plane)
fn linear_depth(d: f32) -> f32 {
    // Reverse-Z with an infinite far plane: depth = near / distance.
    return pc.p1.w / max(d, 1.0e-5);
}

fn decode_octahedral(e: vec2<f32>) -> vec3<f32> {
    let f = e * 2.0 - 1.0;
    var n = vec3<f32>(f.x, f.y, 1.0 - abs(f.x) - abs(f.y));
    if n.z < 0.0 {
        let xy = (1.0 - abs(n.yx)) * select(vec2<f32>(-1.0), vec2<f32>(1.0), n.xy >= vec2<f32>(0.0));
        n = vec3<f32>(xy, n.z);
    }
    return normalize(n);
}

@fragment
fn fs_contours(in: VertexOut) -> @location(0) vec4<f32> {
    let p = vec2<i32>(in.position.xy);
    let color = textureLoad(src, p, 0);
    let t = max(i32(round(pc.p0.x)), 1);
    var gd_x = 0.0;
    var gd_y = 0.0;
    var gn_x = vec3<f32>(0.0);
    var gn_y = vec3<f32>(0.0);
    // Sobel kernels.
    for (var j = -1; j <= 1; j++) {
        for (var i = -1; i <= 1; i++) {
            if i == 0 && j == 0 {
                continue;
            }
            let q = p + vec2<i32>(i, j) * t;
            let wx = f32(i) * select(1.0, 2.0, j == 0);
            let wy = f32(j) * select(1.0, 2.0, i == 0);
            let d = linear_depth(fetch(aux, q).x);
            let n = decode_octahedral(fetch(extra, q).xy);
            gd_x += wx * d;
            gd_y += wy * d;
            gn_x += wx * n;
            gn_y += wy * n;
        }
    }
    let centre = linear_depth(textureLoad(aux, p, 0).x);
    // Depth edges are relative to the distance so far surfaces are not
    // outlined by their sampling noise.
    let depth_edge = length(vec2<f32>(gd_x, gd_y)) / (8.0 * max(centre, 1.0e-3));
    let normal_edge = length(vec2<f32>(length(gn_x), length(gn_y))) / 8.0;
    let e = max(
        smoothstep(pc.p0.z, pc.p0.z * 2.0, depth_edge),
        smoothstep(pc.p0.w, pc.p0.w * 2.0, normal_edge),
    );
    return vec4<f32>(mix(color.rgb, pc.p1.rgb, clamp(e * pc.p0.y, 0.0, 1.0)), color.a);
}

// ------------------------------------------------------------------- bloom

// p0 = (source texel size xy, threshold, knee)
@fragment
fn fs_down(in: VertexOut) -> @location(0) vec4<f32> {
    let uv = in.uv;
    let t = pc.p0.xy;
    // Four bilinear taps: each covers a 2x2 block, together a 4x4 area.
    var c = textureSampleLevel(src, smp, uv + t * vec2<f32>(-1.0, -1.0), 0.0).rgb;
    c += textureSampleLevel(src, smp, uv + t * vec2<f32>(1.0, -1.0), 0.0).rgb;
    c += textureSampleLevel(src, smp, uv + t * vec2<f32>(-1.0, 1.0), 0.0).rgb;
    c += textureSampleLevel(src, smp, uv + t * vec2<f32>(1.0, 1.0), 0.0).rgb;
    c *= 0.25;
    if pc.p0.z > 0.0 || pc.p0.w > 0.0 {
        // Soft threshold: a quadratic knee around the threshold.
        let l = max(c.r, max(c.g, c.b));
        let knee = max(pc.p0.w, 1.0e-4);
        var soft = clamp(l - pc.p0.z + knee, 0.0, 2.0 * knee);
        soft = soft * soft / (4.0 * knee);
        let contribution = max(soft, l - pc.p0.z) / max(l, 1.0e-4);
        c *= contribution;
    }
    return vec4<f32>(min(c, vec3<f32>(65000.0)), 1.0);
}

@fragment
fn fs_up(in: VertexOut) -> @location(0) vec4<f32> {
    let uv = in.uv;
    let t = pc.p0.xy;
    // 3x3 tent filter at the source resolution.
    var c = textureSampleLevel(src, smp, uv, 0.0).rgb * 4.0;
    c += textureSampleLevel(src, smp, uv + t * vec2<f32>(-1.0, 0.0), 0.0).rgb * 2.0;
    c += textureSampleLevel(src, smp, uv + t * vec2<f32>(1.0, 0.0), 0.0).rgb * 2.0;
    c += textureSampleLevel(src, smp, uv + t * vec2<f32>(0.0, -1.0), 0.0).rgb * 2.0;
    c += textureSampleLevel(src, smp, uv + t * vec2<f32>(0.0, 1.0), 0.0).rgb * 2.0;
    c += textureSampleLevel(src, smp, uv + t * vec2<f32>(-1.0, -1.0), 0.0).rgb;
    c += textureSampleLevel(src, smp, uv + t * vec2<f32>(1.0, -1.0), 0.0).rgb;
    c += textureSampleLevel(src, smp, uv + t * vec2<f32>(-1.0, 1.0), 0.0).rgb;
    c += textureSampleLevel(src, smp, uv + t * vec2<f32>(1.0, 1.0), 0.0).rgb;
    return vec4<f32>(c / 16.0, 1.0);
}

// ------------------------------------------------- tonemapping and grading

// Approximate ACES filmic curve (plan form), per channel.
fn aces(x: vec3<f32>) -> vec3<f32> {
    return (x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14);
}

// p0 = (exposure, bloom intensity, tonemap flag, grading flag)
// p1 = lift rgb, saturation   p2 = gamma rgb, hue shift   p3 = gain rgb
@fragment
fn fs_tonemap(in: VertexOut) -> @location(0) vec4<f32> {
    let p = vec2<i32>(in.position.xy);
    var c = textureLoad(src, p, 0).rgb;
    if pc.p0.y > 0.0 {
        c += textureSampleLevel(aux, smp, in.uv, 0.0).rgb * pc.p0.y;
    }
    if pc.p0.z > 0.5 {
        // Exposure, the curve, then a clamp: the curve exceeds 1 above
        // x ~ 7.25, and grading expects a display-range input.
        c = clamp(aces(max(c * pc.p0.x, vec3<f32>(0.0))), vec3<f32>(0.0), vec3<f32>(1.0));
    }
    if pc.p0.w > 0.5 {
        c = c * pc.p3.rgb + pc.p1.rgb;
        c = pow(max(c, vec3<f32>(0.0)), 1.0 / max(pc.p2.rgb, vec3<f32>(1.0e-3)));
        // Hue rotation in YIQ, then saturation around the luma.
        if abs(pc.p2.w) > 1.0e-6 {
            let angle = pc.p2.w;
            let cs = cos(angle);
            let sn = sin(angle);
            let y = dot(c, vec3<f32>(0.299, 0.587, 0.114));
            let i = dot(c, vec3<f32>(0.596, -0.274, -0.322));
            let q = dot(c, vec3<f32>(0.211, -0.523, 0.312));
            let i2 = i * cs - q * sn;
            let q2 = i * sn + q * cs;
            c = vec3<f32>(
                y + 0.956 * i2 + 0.621 * q2,
                y - 0.272 * i2 - 0.647 * q2,
                y - 1.106 * i2 + 1.703 * q2,
        );
        }
        let l = luma(c);
        c = max(mix(vec3<f32>(l), c, pc.p1.w), vec3<f32>(0.0));
    }
    return vec4<f32>(c, 1.0);
}

// ----------------------------------------------------------- screen effects

// p0 = (ripple, chromatic aberration, vignette, grain)
// p1 = (speed lines, damage flash, time, aspect)
// p2 = (flash colour rgb, 0)
@fragment
fn fs_effects(in: VertexOut) -> @location(0) vec4<f32> {
    let p = vec2<i32>(in.position.xy);
    let size = vec2<f32>(textureDimensions(src));
    var uv = in.uv;
    let time = pc.p1.z;
    let centred = uv - vec2<f32>(0.5);
    var c: vec3<f32>;
    let distorted = pc.p0.x > 0.0 || pc.p0.y > 0.0;
    if distorted {
        // Underwater ripple: a slow wobble of the image.
        uv += pc.p0.x * vec2<f32>(
            sin(uv.y * 38.0 + time * 1.7),
            cos(uv.x * 31.0 + time * 1.3),
        );
        // Chromatic aberration: red and blue pulled apart radially.
        let shift = centred * (pc.p0.y * dot(centred, centred) * 4.0 + pc.p0.y * 0.25);
        c = vec3<f32>(
            textureSampleLevel(src, smp, uv + shift, 0.0).r,
            textureSampleLevel(src, smp, uv, 0.0).g,
            textureSampleLevel(src, smp, uv - shift, 0.0).b,
        );
    } else {
        c = textureLoad(src, p, 0).rgb;
    }
    let r2 = dot(centred, centred) * 2.0;
    // Vignette: darkens towards the corners.
    if pc.p0.z > 0.0 {
        c *= 1.0 - pc.p0.z * smoothstep(0.15, 1.0, r2);
    }
    // Speed lines: bright streaks radiating from the centre, at the edges.
    if pc.p1.x > 0.0 {
        let aspect = pc.p1.w;
        let d = vec2<f32>(centred.x * aspect, centred.y);
        let angle = atan2(d.y, d.x);
        let slot = floor(angle * 40.0);
        let streak = step(0.72, hash12(vec2<f32>(slot, floor(time * 24.0))));
        let radial = smoothstep(0.18, 0.5, length(d));
        c += vec3<f32>(streak * radial * pc.p1.x);
    }
    // Damage flash: a tint that is strongest at the edges.
    if pc.p1.y > 0.0 {
        let edge = mix(0.4, 1.0, smoothstep(0.0, 0.8, r2));
        c = mix(c, pc.p2.rgb, clamp(pc.p1.y * edge, 0.0, 1.0));
    }
    // Film grain: zero-mean noise that changes every frame.
    if pc.p0.w > 0.0 {
        let n = hash12(in.position.xy + fract(time) * 97.0) - 0.5;
        c += vec3<f32>(n * pc.p0.w);
    }
    return vec4<f32>(max(c, vec3<f32>(0.0)), 1.0);
}

// -------------------------------------------------------------------- FXAA

fn perceptual_luma(c: vec3<f32>) -> f32 {
    // sqrt approximates the gamma encoding the image will get.
    return luma(sqrt(max(c, vec3<f32>(0.0))));
}

@fragment
fn fs_fxaa(in: VertexOut) -> @location(0) vec4<f32> {
    let p = vec2<i32>(in.position.xy);
    let m = textureLoad(src, p, 0).rgb;
    let nw = fetch(src, p + vec2<i32>(-1, -1)).rgb;
    let ne = fetch(src, p + vec2<i32>(1, -1)).rgb;
    let sw = fetch(src, p + vec2<i32>(-1, 1)).rgb;
    let se = fetch(src, p + vec2<i32>(1, 1)).rgb;
    let l_m = perceptual_luma(m);
    let l_nw = perceptual_luma(nw);
    let l_ne = perceptual_luma(ne);
    let l_sw = perceptual_luma(sw);
    let l_se = perceptual_luma(se);
    let l_min = min(l_m, min(min(l_nw, l_ne), min(l_sw, l_se)));
    let l_max = max(l_m, max(max(l_nw, l_ne), max(l_sw, l_se)));
    // Flat areas and faint edges are left exactly as they are.
    if l_max - l_min < max(0.0312, l_max * 0.125) {
        return vec4<f32>(m, 1.0);
    }
    var dir = vec2<f32>(
        -((l_nw + l_ne) - (l_sw + l_se)),
        (l_nw + l_sw) - (l_ne + l_se),
    );
    let reduce = max((l_nw + l_ne + l_sw + l_se) * 0.25 * 0.125, 1.0 / 128.0);
    let rcp = 1.0 / (min(abs(dir.x), abs(dir.y)) + reduce);
    let texel = 1.0 / vec2<f32>(textureDimensions(src));
    dir = clamp(dir * rcp, vec2<f32>(-8.0), vec2<f32>(8.0)) * texel;
    let uv = in.uv;
    let a = 0.5 * (
        textureSampleLevel(src, smp, uv + dir * (1.0 / 3.0 - 0.5), 0.0).rgb
        + textureSampleLevel(src, smp, uv + dir * (2.0 / 3.0 - 0.5), 0.0).rgb
    );
    let b = a * 0.5 + 0.25 * (
        textureSampleLevel(src, smp, uv + dir * -0.5, 0.0).rgb
        + textureSampleLevel(src, smp, uv + dir * 0.5, 0.0).rgb
    );
    let l_b = perceptual_luma(b);
    if l_b < l_min || l_b > l_max {
        return vec4<f32>(a, 1.0);
    }
    return vec4<f32>(b, 1.0);
}
