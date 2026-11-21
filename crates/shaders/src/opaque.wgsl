// Opaque pass: static meshes drawn as instances.
//
// Outputs: HDR colour (RGBA16F), motion vectors (RG16F, UV units per frame,
// computed without jitter), world normals (RG8, octahedral).
// Albedo and emission are the vertex colour times the procedural material;
// the stylised lighting model and height fog give the final colour.

#include "materials.wgsl"
#include "lighting.wgsl"
#include "wet.wgsl"
#include "caustics.wgsl"

struct Immediates {
    // Sub-pixel jitter in NDC units. Adding `jitter * w` to clip x/y is the
    // same as subtracting it from P[2][0] and P[2][1] (see camera.rs).
    jitter: vec2<f32>,
    // Material animation time in seconds.
    time: f32,
    _pad: f32,
}

var<immediate> imm: Immediates;

struct VertexIn {
    @location(0) position: vec3<f32>,
    // R8G8B8A8_SNORM: xyz normal, w unused.
    @location(1) normal: vec4<f32>,
    // R8G8B8A8_UNORM: rgb colour, a = material id / 255.
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
    // World-from-local affine matrix, rows 0..2.
    @location(11) world0: vec4<f32>,
    @location(12) world1: vec4<f32>,
    @location(13) world2: vec4<f32>,
}

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) clip: vec4<f32>,
    @location(3) prev_clip: vec4<f32>,
    @location(4) world_pos: vec3<f32>,
    @location(5) @interpolate(flat) material: u32,
}

@vertex
fn vs_main(v: VertexIn, i: InstanceIn) -> VertexOut {
    let clip_from_local = mat4x4<f32>(i.clip0, i.clip1, i.clip2, i.clip3);
    let prev_from_local = mat4x4<f32>(i.prev0, i.prev1, i.prev2, i.prev3);
    let local = vec4<f32>(v.position, 1.0);
    let clip = clip_from_local * local;

    // Columns of the linear part; the cofactor matrix is the inverse
    // transpose times the determinant, so its sign fixes mirrored instances.
    let ca = vec3<f32>(i.world0.x, i.world1.x, i.world2.x);
    let cb = vec3<f32>(i.world0.y, i.world1.y, i.world2.y);
    let cc = vec3<f32>(i.world0.z, i.world1.z, i.world2.z);
    let cofactor = mat3x3<f32>(cross(cb, cc), cross(cc, ca), cross(ca, cb));
    let det_sign = sign(dot(ca, cross(cb, cc)));

    var out: VertexOut;
    out.position = vec4<f32>(clip.xy + imm.jitter * clip.w, clip.zw);
    out.color = v.color.rgb;
    out.normal = det_sign * (cofactor * v.normal.xyz);
    out.clip = clip;
    out.prev_clip = prev_from_local * local;
    out.world_pos = vec3<f32>(dot(i.world0, local), dot(i.world1, local), dot(i.world2, local));
    out.material = u32(round(v.color.a * 255.0));
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
    // Derivatives first, in uniform control flow.
    let dpos = fwidth(in.world_pos);
    let normal = normalize(in.normal);
    let surface = material_surface(in.material, in.world_pos, normal, dpos, imm.time);
    // Humidity darkens the surface and adds reflections (a no-op when dry).
    let wet = wet_surface(in.color * surface.albedo, normal, in.world_pos, imm.time);
    let lit = shade(
        wet.albedo,
        in.color * surface.emissive,
        normal,
        in.world_pos,
        in.clip.w,
    ) + wet.albedo * caustic_light(in.world_pos, normal, dpos, imm.time);

    var out: FragmentOut;
    out.color = vec4<f32>(apply_fog(mix(lit, wet.reflection, wet.weight), in.world_pos), 1.0);
    out.motion = 0.5 * (in.clip.xy / in.clip.w - in.prev_clip.xy / in.prev_clip.w);
    out.normal = encode_octahedral(normal);
    return out;
}
