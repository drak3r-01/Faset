// Shading of an opaque fragment, shared by the opaque pass and the
// instanced vegetation (lot 6.11). The including file declares the
// `imm` immediates (`time` is read here).
//
// Outputs: HDR colour (RGBA16F), motion vectors (RG16F, UV units per frame,
// computed without jitter), world normals (RG8, octahedral).
// Albedo and emission are the vertex colour times the procedural material;
// the stylised lighting model and height fog give the final colour.

#include "materials.wgsl"
#include "lighting.wgsl"
#include "wet.wgsl"
#include "caustics.wgsl"

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

// First-person view model (lot 6.9): its depth is squeezed into
// VIEWMODEL_DEPTH_MIN..1 on the CPU (render::viewmodel_projection). A fragment
// in that band is lit with a fixed key light and an ambient term, with no
// shadow, wetness, caustics or fog, so the weapon looks the same on every map.
// World surfaces closer than near / VIEWMODEL_DEPTH_MIN take the same path.
const VIEWMODEL_DEPTH_MIN: f32 = 0.9;

fn shade_viewmodel(albedo: vec3<f32>, emissive: vec3<f32>, n: vec3<f32>) -> vec3<f32> {
    let key = max(dot(n, normalize(vec3<f32>(0.4, 0.8, 0.45))), 0.0);
    let ambient = 0.55 + 0.25 * (0.5 * n.y + 0.5);
    return albedo * (ambient + 0.35 * key) + emissive;
}

// The G-buffer outputs of a fragment. Call it in uniform control flow: the
// materials take screen-space derivatives. `splat_w` is the vertex normal's
// w (0 except on layered terrain, lot 8.1).
//
// On a layered terrain material the vertex colour holds layer weights, not
// a tint: the surface is not multiplied by it, and up close a detail normal
// roughens the light.
fn shade_fragment(
    color_in: vec3<f32>,
    vertex_normal: vec3<f32>,
    splat_w: f32,
    clip: vec4<f32>,
    prev_clip: vec4<f32>,
    world_pos: vec3<f32>,
    material: u32,
) -> FragmentOut {
    // Derivatives first, in uniform control flow.
    let dpos = fwidth(world_pos);
    let layered = materials.entries[min(material, 255u)].pattern == PATTERN_TERRAIN_LAYERS;
    var normal = normalize(vertex_normal);
    var color = color_in;
    if layered {
        normal = terrain_detail_normal(normal, world_pos, clip.w);
        color = vec3<f32>(1.0);
    }
    let surface = material_surface(material, world_pos, normal, dpos, imm.time, vec4<f32>(color_in, splat_w));
    if clip.z / clip.w >= VIEWMODEL_DEPTH_MIN {
        var held: FragmentOut;
        held.color = vec4<f32>(
            shade_viewmodel(color * surface.albedo, color * surface.emissive, normal),
            1.0,
        );
        held.motion = 0.5 * (clip.xy / clip.w - prev_clip.xy / prev_clip.w);
        held.normal = encode_octahedral(normal);
        return held;
    }
    // Humidity darkens the surface and adds reflections (a no-op when dry).
    let wet = wet_surface(color * surface.albedo, normal, world_pos, imm.time);
    let lit = shade(
        wet.albedo,
        color * surface.emissive,
        normal,
        world_pos,
        clip.w,
    ) + wet.albedo * caustic_light(world_pos, normal, dpos, imm.time);

    var out: FragmentOut;
    out.color = vec4<f32>(apply_fog(mix(lit, wet.reflection, wet.weight), world_pos), 1.0);
    out.motion = 0.5 * (clip.xy / clip.w - prev_clip.xy / prev_clip.w);
    out.normal = encode_octahedral(normal);
    return out;
}
