// Cascaded sun shadow pass: depth only. The per-instance clip-from-local
// matrix is the cascade matrix times the world matrix (see
// `render::cull::cull_and_batch`), so no other data is needed.

struct VertexIn {
    @location(0) position: vec3<f32>,
}

struct InstanceIn {
    @location(3) clip0: vec4<f32>,
    @location(4) clip1: vec4<f32>,
    @location(5) clip2: vec4<f32>,
    @location(6) clip3: vec4<f32>,
}

@vertex
fn vs_main(v: VertexIn, i: InstanceIn) -> @builtin(position) vec4<f32> {
    let clip_from_local = mat4x4<f32>(i.clip0, i.clip1, i.clip2, i.clip3);
    return clip_from_local * vec4<f32>(v.position, 1.0);
}
