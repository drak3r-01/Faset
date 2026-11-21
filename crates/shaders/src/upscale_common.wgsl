// Shared by the upscaling passes (`fsr1.wgsl`, `taa.wgsl`, `bilinear.wgsl`).
// Mirrors `faset-upscale`.
//
// Every pass draws one full-screen triangle and reads the images bound at
// set 0; the pass parameters travel in the push constant block (four vec4).

struct Push {
    p0: vec4<f32>,
    p1: vec4<f32>,
    p2: vec4<f32>,
    p3: vec4<f32>,
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

// Largest finite half float, so a stray infinity cannot spread.
const HDR_MAX: f32 = 65000.0;

// Maps linear HDR colour to [0, 1) and back. Filters that expect display
// range input (EASU, RCAS) and the temporal blend work on the compressed
// values, so a very bright pixel cannot ring or flicker the neighbourhood.
fn compress(c: vec3<f32>) -> vec3<f32> {
    let v = clamp(c, vec3<f32>(0.0), vec3<f32>(HDR_MAX));
    return v / (1.0 + max(v.r, max(v.g, v.b)));
}

fn decompress(c: vec3<f32>) -> vec3<f32> {
    let m = min(max(c.r, max(c.g, c.b)), 0.99998);
    return max(c, vec3<f32>(0.0)) / (1.0 - m);
}

fn load_clamped(tex: texture_2d<f32>, p: vec2<i32>) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(tex));
    return textureLoad(tex, clamp(p, vec2<i32>(0), size - vec2<i32>(1)), 0);
}
