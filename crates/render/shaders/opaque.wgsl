// Opaque pass: static meshes drawn as instances.
//
// Outputs: HDR colour (RGBA16F), motion vectors (RG16F, UV units per frame,
// computed without jitter), world normals (RG8, octahedral).
// Lighting arrives in lots 2.3-2.4; colour is the unlit vertex colour.

struct Immediates {
    // Sub-pixel jitter in NDC units. Adding `jitter * w` to clip x/y is the
    // same as subtracting it from P[2][0] and P[2][1] (see camera.rs).
    jitter: vec2<f32>,
}

var<immediate> imm: Immediates;

struct VertexIn {
    @location(0) position: vec3<f32>,
    // R8G8B8A8_SNORM: xyz normal, w unused.
    @location(1) normal: vec4<f32>,
    // R8G8B8A8_UNORM: rgb colour, a = material id / 255 (unused here).
    @location(2) color: vec4<f32>,
}

struct InstanceIn {
    // Unjittered clip-from-local matrix of this frame.
    @location(3) clip0: vec4<f32>,
    @location(4) clip1: vec4<f32>,
    @location(5) clip2: vec4<f32>,
    @location(6) clip3: vec4<f32>,
    // Clip-from-local matrix of the previous frame.
    @location(7) prev0: vec4<f32>,
    @location(8) prev1: vec4<f32>,
    @location(9) prev2: vec4<f32>,
    @location(10) prev3: vec4<f32>,
    // World-from-local normal matrix (inverse transpose), xyz columns.
    @location(11) n0: vec4<f32>,
    @location(12) n1: vec4<f32>,
    @location(13) n2: vec4<f32>,
}

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) clip: vec4<f32>,
    @location(3) prev_clip: vec4<f32>,
}

@vertex
fn vs_main(v: VertexIn, i: InstanceIn) -> VertexOut {
    let clip_from_local = mat4x4<f32>(i.clip0, i.clip1, i.clip2, i.clip3);
    let prev_from_local = mat4x4<f32>(i.prev0, i.prev1, i.prev2, i.prev3);
    let normal_matrix = mat3x3<f32>(i.n0.xyz, i.n1.xyz, i.n2.xyz);
    let local = vec4<f32>(v.position, 1.0);
    let clip = clip_from_local * local;

    var out: VertexOut;
    out.position = vec4<f32>(clip.xy + imm.jitter * clip.w, clip.zw);
    out.color = v.color.rgb;
    out.normal = normal_matrix * v.normal.xyz;
    out.clip = clip;
    out.prev_clip = prev_from_local * local;
    return out;
}

struct FragmentOut {
    @location(0) color: vec4<f32>,
    @location(1) motion: vec2<f32>,
    @location(2) normal: vec2<f32>,
}

// Octahedral encoding of a unit vector into [0, 1]^2.
fn encode_octahedral(n: vec3<f32>) -> vec2<f32> {
    let p = n.xy / (abs(n.x) + abs(n.y) + abs(n.z));
    var e = p;
    if n.z < 0.0 {
        e = (1.0 - abs(p.yx)) * select(vec2<f32>(-1.0), vec2<f32>(1.0), p >= vec2<f32>(0.0));
    }
    return e * 0.5 + 0.5;
}

@fragment
fn fs_main(in: VertexOut) -> FragmentOut {
    var out: FragmentOut;
    out.color = vec4<f32>(in.color, 1.0);
    out.motion = 0.5 * (in.clip.xy / in.clip.w - in.prev_clip.xy / in.prev_clip.w);
    out.normal = encode_octahedral(normalize(in.normal));
    return out;
}
