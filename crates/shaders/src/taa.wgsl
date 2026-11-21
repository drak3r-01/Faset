// Temporal anti-aliasing resolve. Mirrors `faset-upscale::taa` (the CPU
// reference of the same maths is tested there).
//
// The current frame is jittered (Halton 2, 3); its motion vectors come from
// the unjittered matrices. For each pixel:
//   1. the 3x3 neighbourhood of the current colour gives a bounding box in
//      YCoCg;
//   2. the history is fetched where the pixel was last frame (the motion of
//      the closest-depth pixel of the 3x3, so edges follow the object in
//      front) and clamped to the box, which removes ghosts;
//   3. the current sample is blended in at `p0.w` (10 %), or entirely where
//      the history is invalid (reset, off screen) or weather particles cover
//      the pixel (reactive mask).
// Colours are compressed to [0, 1) (`c / (1 + max(c))`) before blending so
// HDR highlights do not flicker; the history holds the compressed result,
// the output the expanded one.

#include "upscale_common.wgsl"

@group(0) @binding(0) var cur_tex: texture_2d<f32>;
@group(0) @binding(1) var hist_tex: texture_2d<f32>;
@group(0) @binding(2) var depth_tex: texture_2d<f32>;
@group(0) @binding(3) var motion_tex: texture_2d<f32>;
@group(0) @binding(4) var reactive_tex: texture_2d<f32>;
@group(0) @binding(5) var smp: sampler;

// p0 = (1 / width, 1 / height, 1 when the history is valid, current weight)
// p1 = (1 when the reactive mask is bound, 0, 0, 0)

struct FragmentOut {
    @location(0) colour: vec4<f32>,
    @location(1) history: vec4<f32>,
}

fn rgb_to_ycocg(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        0.25 * c.r + 0.5 * c.g + 0.25 * c.b,
        0.5 * c.r - 0.5 * c.b,
        -0.25 * c.r + 0.5 * c.g - 0.25 * c.b,
    );
}

fn ycocg_to_rgb(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(c.x + c.y - c.z, c.x + c.z, c.x - c.y - c.z);
}

@fragment
fn fs_main(in: VertexOut) -> FragmentOut {
    let p = vec2<i32>(in.position.xy);
    let uv = (vec2<f32>(p) + vec2<f32>(0.5)) * pc.p0.xy;

    var box_min = vec3<f32>(1.0e9);
    var box_max = vec3<f32>(-1.0e9);
    var closest = -1.0;
    var closest_offset = vec2<i32>(0);
    var current = vec3<f32>(0.0);
    for (var j = -1; j <= 1; j++) {
        for (var i = -1; i <= 1; i++) {
            let q = p + vec2<i32>(i, j);
            let c = compress(load_clamped(cur_tex, q).rgb);
            if i == 0 && j == 0 {
                current = c;
            }
            let y = rgb_to_ycocg(c);
            box_min = min(box_min, y);
            box_max = max(box_max, y);
            // Reverse-Z: the larger depth is the closer surface.
            let d = load_clamped(depth_tex, q).x;
            if d > closest {
                closest = d;
                closest_offset = vec2<i32>(i, j);
            }
        }
    }

    let motion = load_clamped(motion_tex, p + closest_offset).xy;
    let prev_uv = uv - motion;
    let on_screen = all(prev_uv >= vec2<f32>(0.0)) && all(prev_uv <= vec2<f32>(1.0));
    var weight = pc.p0.w;
    if pc.p0.z < 0.5 || !on_screen {
        weight = 1.0;
    }
    // Particles are not in the motion vectors: trust the history less.
    if pc.p1.x > 0.5 && weight < 1.0 {
        weight = max(weight, clamp(textureLoad(reactive_tex, p, 0).x, 0.0, 1.0));
    }

    let history = textureSampleLevel(hist_tex, smp, clamp(prev_uv, vec2<f32>(0.0), vec2<f32>(1.0)), 0.0).rgb;
    let clamped = ycocg_to_rgb(clamp(rgb_to_ycocg(history), box_min, box_max));
    let result = mix(clamped, current, weight);

    var out: FragmentOut;
    out.colour = vec4<f32>(decompress(result), 1.0);
    out.history = vec4<f32>(result, 1.0);
    return out;
}
