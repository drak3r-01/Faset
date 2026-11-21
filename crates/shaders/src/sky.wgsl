// Procedural sky: one full-screen triangle drawn after the opaque pass, only
// where the depth buffer is still empty (reverse-Z clear value 0).
//
// The colour depends on the view direction v only:
//   C_sky(v) = mix(C_horizon, C_zenith, max(v.y, 0)^k)
// plus the sun disc and its Henyey-Greenstein halo
//   P(theta) = (1 - g^2) / (4 pi (1 + g^2 - 2 g cos(theta))^(3/2)), g = 0.76,
// the moon disc, stars hashed on a grid of directions, one layer of
// scrolling cloud noise thresholded by the cloud cover and, in a space
// profile, a nebula and shaded planet discs. Mirrors `render::sky`.
//
// Outputs match the opaque pass: HDR colour, motion vectors of a point at
// infinity (camera rotation only), and the clear value of the normal buffer.

#include "noise.wgsl"
#include "lighting.wgsl"

struct Immediates {
    // Sub-pixel jitter in NDC units (see `opaque.wgsl`).
    jitter: vec2<f32>,
    // Animation time in seconds: star twinkle and cloud scrolling.
    time: f32,
    _pad: f32,
}

var<immediate> imm: Immediates;

const PI: f32 = 3.14159265;
const TAU: f32 = 6.2831853;

// Must match `render::sky`.
const SUN_PHASE_G: f32 = 0.76;
const SUN_HALO: f32 = 0.12;
const SUN_DISC_RADIUS: f32 = 0.020;
const SUN_DISC_BRIGHTNESS: f32 = 8.0;
const MOON_RADIUS: f32 = 0.030;
const STAR_COLS: i32 = 200;
const STAR_ROWS: i32 = 64;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) ndc: vec2<f32>,
}

// Full-screen triangle at the far plane (reverse-Z depth 0).
@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOut {
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    let p = corner * 2.0 - vec2<f32>(1.0);
    var out: VertexOut;
    out.position = vec4<f32>(p, 0.0, 1.0);
    out.ndc = p;
    return out;
}

// Henyey-Greenstein phase function; integrates to 1 over the sphere.
fn henyey_greenstein(cos_theta: f32, g: f32) -> f32 {
    let denom = 1.0 + g * g - 2.0 * g * cos_theta;
    return (1.0 - g * g) / (4.0 * PI * denom * sqrt(denom));
}

fn luminance(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// Stars: the sphere is cut into STAR_ROWS equal-height bands (equal-area
// cylindrical projection) of STAR_COLS cells; a cell holds a star when its
// hash is below `density`. The star sits at a hashed point inside the cell.
// `px` is the angular size of a pixel, used to keep stars at least about a
// pixel wide.
fn star_field(v: vec3<f32>, density: f32, px: f32) -> vec3<f32> {
    let col_f = (atan2(v.z, v.x) / TAU + 0.5) * f32(STAR_COLS);
    let row_f = (v.y * 0.5 + 0.5) * f32(STAR_ROWS);
    let base = vec2<i32>(i32(floor(col_f)), i32(floor(row_f)));
    var sum = vec3<f32>(0.0);
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            var cell = base + vec2<i32>(dx, dy);
            if cell.y < 0 || cell.y >= STAR_ROWS {
                continue;
            }
            cell.x = ((cell.x % STAR_COLS) + STAR_COLS) % STAR_COLS;
            if hash2(cell) >= density {
                continue;
            }
            let jx = hash2(cell + vec2<i32>(1013, 7));
            let jy = hash2(cell + vec2<i32>(29, 3571));
            let magnitude = hash2(cell + vec2<i32>(5, 8191));
            let tint = hash2(cell + vec2<i32>(77, 4001));
            let phi = ((f32(cell.x) + 0.15 + 0.7 * jx) / f32(STAR_COLS) - 0.5) * TAU;
            let y = ((f32(cell.y) + 0.15 + 0.7 * jy) / f32(STAR_ROWS)) * 2.0 - 1.0;
            let r = sqrt(max(1.0 - y * y, 0.0));
            let s = vec3<f32>(r * cos(phi), y, r * sin(phi));
            let chord = length(v - s);
            let size = 0.0010 + 0.0016 * magnitude * magnitude;
            let radius = max(size, 0.9 * px);
            let falloff = 1.0 - smoothstep(0.0, radius, chord);
            // Dimmer when the star is smaller than a pixel.
            let coverage = clamp(size / radius, 0.5, 1.0);
            let twinkle = 0.8 + 0.2 * sin(imm.time * (1.5 + 3.0 * magnitude) + TAU * tint);
            let colour = mix(vec3<f32>(1.0, 0.80, 0.62), vec3<f32>(0.72, 0.82, 1.0), tint);
            let brightness = 0.3 + 0.7 * magnitude * magnitude * magnitude;
            sum += colour * (brightness * falloff * falloff * coverage * twinkle * 4.0);
        }
    }
    return sum;
}

// Five octaves of 2D value noise in [0, 1].
fn fbm2(p: vec2<f32>) -> f32 {
    var sum = 0.0;
    var amplitude = 0.5;
    var q = p;
    for (var octave = 0; octave < 5; octave++) {
        sum += amplitude * value_noise2(q);
        q = q * 2.03 + vec2<f32>(17.0, 31.0);
        amplitude *= 0.5;
    }
    return sum / 0.96875;
}

// Cloud layer over `sky` in direction `v` (v.y > 0): the direction is
// projected on a plane above the viewer, the noise scrolls with the wind and
// the cover thresholds it (0 = clear, 1 = overcast).
fn add_clouds(sky: vec3<f32>, v: vec3<f32>) -> vec3<f32> {
    let cover = lighting.sky_params.x;
    let plane = v.xz / (v.y + 0.12);
    let p = plane * 1.6 + lighting.sky_wind.xy * (imm.time * 0.05);
    let density = fbm2(p);
    let threshold = 1.0 - cover * 0.95;
    let thick = smoothstep(threshold, threshold + 0.18, density);
    let alpha = thick * smoothstep(0.0, 0.18, v.y) * (1.0 - lighting.sky_params.z);
    let daylight = lighting.sky_wind.z;
    let mid = mix(lighting.sky_horizon.rgb, lighting.sky_zenith.rgb, 0.5);
    let body = mid * 0.5 + vec3<f32>(0.65) * daylight;
    let dense = smoothstep(threshold, threshold + 0.45, density);
    let sun_lit = lighting.sun_color.rgb * (0.7 * pow(max(dot(v, lighting.sun_dir.xyz), 0.0), 6.0));
    let colour = body * (1.0 - 0.3 * dense) + sun_lit;
    return mix(sky, colour, alpha * 0.92);
}

// A shaded sphere of angular radius `radius` around unit direction `c`,
// lit by the sun. Returns rgb in xyz and the coverage in w.
fn planet(
    v: vec3<f32>,
    c: vec3<f32>,
    radius: f32,
    base: vec3<f32>,
    accent: vec3<f32>,
    bands: f32,
    px: f32,
) -> vec4<f32> {
    let rho = sin(radius);
    let b = dot(v, c);
    let disc = b * b - (1.0 - rho * rho);
    if disc <= 0.0 || b <= 0.0 {
        return vec4<f32>(0.0);
    }
    let n = normalize(v * (b - sqrt(disc)) - c);
    let edge = clamp(disc / (2.0 * rho) / max(1.5 * px, 1.0e-6), 0.0, 1.0);
    let stripe = 0.5 + 0.5 * sin(n.y * bands * 3.0 + 2.5 * value_noise2(vec2<f32>(n.x * 3.0, n.y * bands)));
    let albedo = mix(base, accent, stripe);
    let lit = smoothstep(-0.05, 0.3, dot(n, lighting.sun_dir.xyz));
    let atmosphere = pow(1.0 - max(dot(n, -v), 0.0), 3.0);
    let colour = albedo * (0.03 + lit * lighting.sun_color.rgb) + accent * atmosphere * (0.1 + 0.4 * lit);
    return vec4<f32>(colour, edge);
}

// Nebula of fractal noise, only in space profiles.
fn nebula(v: vec3<f32>, footprint: f32) -> vec3<f32> {
    let p = v * 2.2 + vec3<f32>(5.0, 1.0, 9.0);
    let n1 = fbm3(p, footprint * 2.2);
    let n2 = fbm3(p * 1.7 + vec3<f32>(11.0), footprint * 3.7);
    let cloud = smoothstep(0.48, 0.80, n1);
    let colour = mix(vec3<f32>(0.04, 0.26, 0.42), vec3<f32>(0.38, 0.09, 0.50), smoothstep(0.3, 0.7, n2));
    return colour * (cloud * 0.4);
}

fn sky_color(v: vec3<f32>, px: f32) -> vec3<f32> {
    let space = lighting.sky_params.z;
    // Sun and moon are hidden under the horizon, except in space.
    let above = mix(smoothstep(-0.02, 0.02, v.y), 1.0, space);
    var col = sky_gradient(v);

    // Stars.
    let star_brightness = lighting.sky_zenith.w;
    if star_brightness > 0.0 {
        col += star_field(v, lighting.sky_params.y, px) * (star_brightness * above);
    }

    // Nebula and planets (space profiles).
    if space > 0.0 {
        col += nebula(v, px) * space;
        var planets = planet(v, normalize(vec3<f32>(-0.25, 0.20, -0.95)), 0.17,
            vec3<f32>(0.55, 0.36, 0.22), vec3<f32>(0.85, 0.68, 0.45), 9.0, px);
        col = mix(col, planets.rgb, planets.a * space);
        planets = planet(v, normalize(vec3<f32>(0.38, 0.12, -0.92)), 0.07,
            vec3<f32>(0.12, 0.28, 0.62), vec3<f32>(0.40, 0.62, 0.90), 3.0, px);
        col = mix(col, planets.rgb, planets.a * space);
        planets = planet(v, normalize(vec3<f32>(0.05, 0.28, -0.96)), 0.03,
            vec3<f32>(0.40, 0.40, 0.42), vec3<f32>(0.55, 0.52, 0.50), 1.0, px);
        col = mix(col, planets.rgb, planets.a * space);
    }

    // Clouds.
    if lighting.sky_params.x > 0.0 && v.y > 0.0 {
        col = add_clouds(col, v);
    }

    // Moon: a pale disc with a faint glow.
    let moon_dir = lighting.sky_moon.xyz;
    let moon_cos = dot(v, moon_dir);
    let moon_disc = smoothstep(cos(MOON_RADIUS * 1.12), cos(MOON_RADIUS), moon_cos);
    let craters = 0.82 + 0.18 * fbm3(v * 90.0, px * 90.0);
    let moon_colour = vec3<f32>(0.80, 0.84, 0.95);
    col += (moon_colour * (1.6 * craters * moon_disc) + moon_colour * (0.10 * pow(max(moon_cos, 0.0), 250.0)))
        * (lighting.sky_moon.w * above);

    // Sun: Henyey-Greenstein halo and a bright disc.
    let sun_cos = dot(v, lighting.sun_dir.xyz);
    let sun_disc = smoothstep(cos(SUN_DISC_RADIUS * 1.15), cos(SUN_DISC_RADIUS), sun_cos);
    col += lighting.sun_color.rgb * (SUN_HALO * henyey_greenstein(sun_cos, SUN_PHASE_G) + SUN_DISC_BRIGHTNESS * sun_disc) * above;
    return max(col, vec3<f32>(0.0));
}

struct FragmentOut {
    @location(0) color: vec4<f32>,
    @location(1) motion: vec2<f32>,
    @location(2) normal: vec2<f32>,
}

@fragment
fn fs_main(in: VertexOut) -> FragmentOut {
    // The opaque pass shifts geometry by the jitter; undo it here so the
    // sky is jittered like everything else.
    let ndc = in.ndc - imm.jitter;
    let ray = vec3<f32>(ndc.x * lighting.sky_ray.x, ndc.y * lighting.sky_ray.y, -1.0);
    let v = normalize((lighting.sky_world_from_view * vec4<f32>(ray, 0.0)).xyz);
    // Angular size of a pixel, from derivatives taken in uniform control flow.
    let px = max(length(dpdx(v)), length(dpdy(v)));

    // The sky is at infinity: its previous clip position ignores translation.
    let prev = lighting.sky_prev_view_proj * vec4<f32>(v, 0.0);
    var motion = vec2<f32>(0.0);
    if prev.w > 1.0e-6 {
        motion = 0.5 * (ndc - prev.xy / prev.w);
    }

    var out: FragmentOut;
    out.color = vec4<f32>(sky_color(v, px), 1.0);
    out.motion = motion;
    out.normal = vec2<f32>(0.5, 0.5);
    return out;
}
