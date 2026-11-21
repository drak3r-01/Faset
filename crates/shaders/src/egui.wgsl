// egui interface pass (pass 9), drawn at the display resolution onto an sRGB
// target. Mirrors `faset-ui`.
//
// Vertices come from egui in points; the push constants carry the screen size
// in points. Vertex colours and textures are sRGB-encoded (premultiplied
// alpha): like egui expects, they are multiplied in that encoding, and the
// product is converted to linear because the target converts back when it is
// written.

struct Push {
    screen: vec2<f32>,
}

var<immediate> pc: Push;

struct VertexIn {
    @location(0) pos: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
}

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let low = c / 12.92;
    let high = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(high, low, c <= vec3<f32>(0.04045));
}

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    var out: VertexOut;
    out.position = vec4<f32>(
        2.0 * in.pos.x / pc.screen.x - 1.0,
        2.0 * in.pos.y / pc.screen.y - 1.0,
        0.0,
        1.0,
    );
    out.uv = in.uv;
    out.color = in.color;
    return out;
}

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let c = in.color * textureSample(tex, smp, in.uv);
    return vec4<f32>(srgb_to_linear(c.rgb), c.a);
}
