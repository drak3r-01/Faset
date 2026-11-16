// Integer hashes, value noise and box-filtered periodic functions.
//
// Hashes use integer arithmetic so every GPU gives the same bits.

// PCG hash of one 32-bit word.
fn hash_u32(v: u32) -> u32 {
    let state = v * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

// Hash of an integer cell in [0, 1].
fn hash2(c: vec2<i32>) -> f32 {
    let h = hash_u32(bitcast<u32>(c.x) ^ hash_u32(bitcast<u32>(c.y) + 0x9e3779b9u));
    return f32(h >> 8u) * (1.0 / 16777215.0);
}

fn hash3(c: vec3<i32>) -> f32 {
    let h = hash_u32(bitcast<u32>(c.x) ^ hash_u32(bitcast<u32>(c.y) ^ hash_u32(bitcast<u32>(c.z))));
    return f32(h >> 8u) * (1.0 / 16777215.0);
}

// 2D value noise in [0, 1], smooth (quintic) interpolation.
fn value_noise2(p: vec2<f32>) -> f32 {
    let i = vec2<i32>(floor(p));
    let f = fract(p);
    let u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    let a = hash2(i);
    let b = hash2(i + vec2<i32>(1, 0));
    let c = hash2(i + vec2<i32>(0, 1));
    let d = hash2(i + vec2<i32>(1, 1));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

// 3D value noise in [0, 1].
fn value_noise3(p: vec3<f32>) -> f32 {
    let i = vec3<i32>(floor(p));
    let f = fract(p);
    let u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    let x00 = mix(hash3(i), hash3(i + vec3<i32>(1, 0, 0)), u.x);
    let x10 = mix(hash3(i + vec3<i32>(0, 1, 0)), hash3(i + vec3<i32>(1, 1, 0)), u.x);
    let x01 = mix(hash3(i + vec3<i32>(0, 0, 1)), hash3(i + vec3<i32>(1, 0, 1)), u.x);
    let x11 = mix(hash3(i + vec3<i32>(0, 1, 1)), hash3(i + vec3<i32>(1, 1, 1)), u.x);
    return mix(mix(x00, x10, u.y), mix(x01, x11, u.y), u.z);
}

// Weight of a detail whose period is `period` cells when one pixel covers
// `footprint` cells: 1 when well resolved, 0 below two pixels per period.
fn detail_weight(footprint: f32, period: f32) -> f32 {
    return 1.0 - smoothstep(0.25, 0.5, footprint / period);
}

// Four octaves of 3D value noise in [0, 1], mean 0.5. Octaves finer than
// the pixel footprint (in cells) fade to the mean.
fn fbm3(p: vec3<f32>, footprint: f32) -> f32 {
    var sum = 0.0;
    var amplitude = 0.5;
    var frequency = 1.0;
    for (var octave = 0; octave < 4; octave++) {
        let k = detail_weight(footprint * frequency, 1.0);
        sum += amplitude * k * (value_noise3(p * frequency + f32(octave) * 17.0) - 0.5);
        amplitude *= 0.5;
        frequency *= 2.0;
    }
    return 0.5 + sum / 0.9375;
}

// Box-filtered square wave of period 2 (1 on [0, 1), 0 on [1, 2)) over a
// footprint of width `w`: the mean value of the wave on [x - w/2, x + w/2].
fn filtered_square(x: f32, w: f32) -> f32 {
    let ww = max(w, 1.0e-4);
    let i = 2.0 * (abs(fract((x - 0.5 * ww) * 0.5) - 0.5) - abs(fract((x + 0.5 * ww) * 0.5) - 0.5)) / ww;
    return 0.5 - 0.5 * i;
}

// Box-filtered pulse train: 1 on [k, k + width) for every integer k, over a
// footprint of width `w`. The exact mean of the pulses on the footprint.
fn filtered_lines(x: f32, width: f32, w: f32) -> f32 {
    let ww = max(w, 1.0e-4);
    let a = x - 0.5 * ww;
    let b = x + 0.5 * ww;
    let fa = floor(a) * width + min(fract(a), width);
    let fb = floor(b) * width + min(fract(b), width);
    return (fb - fa) / ww;
}
