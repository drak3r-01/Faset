// Plain bilinear resampling to the display size: the "Native" upscaler at a
// render scale below 100 %. Mirrors `faset-upscale`.

#include "upscale_common.wgsl"

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    return vec4<f32>(textureSampleLevel(src, smp, in.uv, 0.0).rgb, 1.0);
}
