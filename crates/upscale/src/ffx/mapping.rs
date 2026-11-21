//! From the renderer's [`UpscaleInputs`] to the descriptors of the FidelityFX
//! API: context creation, per-frame dispatch, resources and the jitter
//! sequence. Plain data, no call into the library.

use ash::vk;
use faset_render::glam::Vec2;
use faset_render::{ImageHandle, UpscaleInputs};

use super::api::raw_handle;
use super::bindings as ffx;
use crate::Extent2D;

/// The library file: the FidelityFX API with its Vulkan backend
/// (`PrebuiltSignedDLL/amd_fidelityfx_vk.dll` of the FidelityFX SDK v1.1.4).
pub(crate) const LIBRARY_NAME: &str = "amd_fidelityfx_vk.dll";

/// Decision: FSR 3's own RCAS sharpening is on, at half strength.
pub(crate) const SHARPNESS: f32 = 0.5;

/// Size of the context created to check that the library initialises on the
/// device.
pub(crate) const PROBE_EXTENT: Extent2D = Extent2D {
    width: 64,
    height: 64,
};

/// The renderer's device, as the Vulkan backend of the library takes it.
#[derive(Clone, Copy)]
pub(crate) struct FfxDevice {
    pub device: vk::Device,
    pub physical: vk::PhysicalDevice,
    pub get_device_proc_addr: vk::PFN_vkGetDeviceProcAddr,
}

/// Context flags: linear HDR colour before tonemapping, and the reverse-Z
/// infinite depth of lot 2.2 (`z_ndc = n / -z_view`: near = 1, infinity = 0).
/// The motion vectors come from the unjittered matrices and the exposure is
/// given, so neither jitter cancellation nor auto-exposure is asked for.
pub(crate) fn create_flags() -> u32 {
    let mut flags = ffx::FFX_UPSCALE_ENABLE_HIGH_DYNAMIC_RANGE
        | ffx::FFX_UPSCALE_ENABLE_DEPTH_INVERTED
        | ffx::FFX_UPSCALE_ENABLE_DEPTH_INFINITE;
    if cfg!(debug_assertions) {
        flags |= ffx::FFX_UPSCALE_ENABLE_DEBUG_CHECKING;
    }
    flags
}

/// The two descriptors of context creation: the upscaling effect, chained to
/// the Vulkan backend.
pub(crate) struct CreateDescs {
    pub upscale: ffx::ffxCreateContextDescUpscale,
    pub backend: ffx::ffxCreateBackendVKDesc,
}

impl CreateDescs {
    pub(crate) fn new(device: &FfxDevice, display: Extent2D) -> Self {
        let size = dims(display);
        Self {
            upscale: ffx::ffxCreateContextDescUpscale {
                header: header(ffx::FFX_API_CREATE_CONTEXT_DESC_TYPE_UPSCALE),
                flags: create_flags(),
                // Up to native resolution (quality "Native" is native AA).
                maxRenderSize: size,
                maxUpscaleSize: size,
                fpMessage: Some(super::api::on_message),
            },
            backend: ffx::ffxCreateBackendVKDesc {
                header: header(ffx::FFX_API_CREATE_CONTEXT_DESC_TYPE_BACKEND_VK),
                vkDevice: device.device,
                vkPhysicalDevice: device.physical,
                vkDeviceProcAddr: Some(device.get_device_proc_addr),
            },
        }
    }

    /// Links the chain and returns its head, for `ffxCreateContext`. The
    /// pointer is valid while `self` is neither moved nor dropped.
    pub(crate) fn chain(&mut self) -> *mut ffx::ffxApiHeader {
        self.upscale.header.pNext = &mut self.backend.header;
        &mut self.upscale.header
    }
}

fn header(ty: u32) -> ffx::ffxApiHeader {
    ffx::ffxApiHeader {
        type_: u64::from(ty),
        pNext: std::ptr::null_mut(),
    }
}

fn dims(e: Extent2D) -> ffx::FfxApiDimensions2D {
    ffx::FfxApiDimensions2D {
        width: e.width,
        height: e.height,
    }
}

/// The FidelityFX surface format of the Vulkan formats the renderer hands
/// over (the matching subset of `ffxApiGetSurfaceFormatVK`).
pub(crate) fn surface_format(format: vk::Format) -> u32 {
    match format {
        vk::Format::R16G16B16A16_SFLOAT => ffx::FFX_API_SURFACE_FORMAT_R16G16B16A16_FLOAT,
        vk::Format::R32G32B32A32_SFLOAT => ffx::FFX_API_SURFACE_FORMAT_R32G32B32A32_FLOAT,
        vk::Format::B10G11R11_UFLOAT_PACK32 => ffx::FFX_API_SURFACE_FORMAT_R11G11B10_FLOAT,
        vk::Format::R16G16_SFLOAT => ffx::FFX_API_SURFACE_FORMAT_R16G16_FLOAT,
        vk::Format::R32G32_SFLOAT => ffx::FFX_API_SURFACE_FORMAT_R32G32_FLOAT,
        vk::Format::R32_SFLOAT | vk::Format::D32_SFLOAT => ffx::FFX_API_SURFACE_FORMAT_R32_FLOAT,
        vk::Format::R16_SFLOAT => ffx::FFX_API_SURFACE_FORMAT_R16_FLOAT,
        vk::Format::R8_UNORM => ffx::FFX_API_SURFACE_FORMAT_R8_UNORM,
        vk::Format::R8G8B8A8_UNORM => ffx::FFX_API_SURFACE_FORMAT_R8G8B8A8_UNORM,
        _ => ffx::FFX_API_SURFACE_FORMAT_UNKNOWN,
    }
}

/// A 2D, single-mip image as a FidelityFX resource in `state`.
pub(crate) fn texture(image: &ImageHandle, usage: u32, state: u32) -> ffx::FfxApiResource {
    ffx::FfxApiResource {
        resource: raw_handle(image.image),
        description: ffx::FfxApiResourceDescription {
            type_: ffx::FFX_API_RESOURCE_TYPE_TEXTURE2D,
            format: surface_format(image.format),
            __bindgen_anon_1: ffx::FfxApiResourceDescription__bindgen_ty_1 {
                width: image.extent.width,
            },
            __bindgen_anon_2: ffx::FfxApiResourceDescription__bindgen_ty_2 {
                height: image.extent.height,
            },
            // One array layer.
            __bindgen_anon_3: ffx::FfxApiResourceDescription__bindgen_ty_3 { depth: 1 },
            mipCount: 1,
            flags: ffx::FFX_API_RESOURCE_FLAGS_NONE,
            usage,
        },
        state,
    }
}

/// An input read by the compute passes. The library reads the layout from
/// the state: `COMPUTE_READ` is `SHADER_READ_ONLY_OPTIMAL`, and an external
/// resource is put back in its state at the end of the dispatch.
fn input(image: &ImageHandle) -> ffx::FfxApiResource {
    texture(
        image,
        ffx::FFX_API_RESOURCE_USAGE_READ_ONLY,
        ffx::FFX_API_RESOURCE_STATE_COMPUTE_READ,
    )
}

/// The per-frame dispatch of the upscaler.
///
/// * `output`: display-sized storage image in `GENERAL` (`UNORDERED_ACCESS`).
/// * `exposure`: a 1 x 1 image holding `inputs.exposure` in its red channel.
/// * The depth is read with a depth view; the caller moves it to
///   `SHADER_READ_ONLY_OPTIMAL` around the dispatch.
/// * The motion vectors are `current - previous` in UV units; the library
///   wants `previous - current` in render pixels, hence the negative scale of
///   the render size.
/// * Frame time in milliseconds; the colour is not pre-exposed (1.0); one
///   view-space unit is one metre.
/// * The jitter and the reset flag are passed exactly as given.
pub(crate) fn dispatch_desc(
    inputs: &UpscaleInputs,
    cmd: vk::CommandBuffer,
    output: &ImageHandle,
    exposure: &ImageHandle,
) -> ffx::ffxDispatchDescUpscale {
    let render = inputs.colour.extent;
    ffx::ffxDispatchDescUpscale {
        header: header(ffx::FFX_API_DISPATCH_DESC_TYPE_UPSCALE),
        commandList: raw_handle(cmd),
        color: input(&inputs.colour),
        depth: texture(
            &inputs.depth,
            ffx::FFX_API_RESOURCE_USAGE_DEPTHTARGET,
            ffx::FFX_API_RESOURCE_STATE_COMPUTE_READ,
        ),
        motionVectors: input(&inputs.motion),
        exposure: input(exposure),
        reactive: inputs.reactive.as_ref().map(input).unwrap_or_default(),
        transparencyAndComposition: ffx::FfxApiResource::default(),
        output: texture(
            output,
            ffx::FFX_API_RESOURCE_USAGE_UAV,
            ffx::FFX_API_RESOURCE_STATE_UNORDERED_ACCESS,
        ),
        jitterOffset: ffx::FfxApiFloatCoords2D {
            x: inputs.jitter.x,
            y: inputs.jitter.y,
        },
        motionVectorScale: ffx::FfxApiFloatCoords2D {
            x: -(render.width as f32),
            y: -(render.height as f32),
        },
        renderSize: dims(render),
        upscaleSize: dims(inputs.display),
        enableSharpening: true,
        sharpness: SHARPNESS,
        frameTimeDelta: inputs.frame_time * 1000.0,
        preExposure: 1.0,
        reset: inputs.reset,
        cameraNear: inputs.near,
        // With an infinite depth only the near plane is used; the library
        // expects a finite number.
        cameraFar: if inputs.far.is_finite() {
            inputs.far
        } else {
            f32::MAX
        },
        cameraFovAngleVertical: inputs.fov_y,
        viewSpaceToMetersFactor: 1.0,
        flags: 0,
    }
}

/// Length of the jitter sequence for a scaling ratio, as
/// `ffxQueryDescUpscaleGetJitterPhaseCount`: `8 * (display / render)^2`.
pub(crate) fn jitter_phase_count(render_width: u32, display_width: u32) -> u32 {
    let ratio = display_width.max(1) as f32 / render_width.max(1) as f32;
    ((8.0 * ratio * ratio) as u32).max(1)
}

/// Jitter of a frame in render pixels, as `ffxQueryDescUpscaleGetJitterOffset`:
/// Halton (2, 3) at `index % phases + 1`, centred on zero.
pub(crate) fn jitter_offset(index: u32, phases: u32) -> Vec2 {
    let k = index % phases.max(1) + 1;
    Vec2::new(halton(k, 2) - 0.5, halton(k, 3) - 0.5)
}

fn halton(mut index: u32, base: u32) -> f32 {
    let mut f = 1.0f32;
    let mut r = 0.0f32;
    while index > 0 {
        f /= base as f32;
        r += f * (index % base) as f32;
        index /= base;
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(width: u32, height: u32, format: vk::Format, raw: u64) -> ImageHandle {
        use ash::vk::Handle;
        ImageHandle {
            image: vk::Image::from_raw(raw),
            view: vk::ImageView::null(),
            extent: vk::Extent2D { width, height },
            format,
        }
    }

    fn inputs(reset: bool, jitter: Vec2) -> UpscaleInputs {
        UpscaleInputs {
            colour: image(1280, 720, vk::Format::R16G16B16A16_SFLOAT, 1),
            depth: image(1280, 720, vk::Format::D32_SFLOAT, 2),
            motion: image(1280, 720, vk::Format::R16G16_SFLOAT, 3),
            reactive: Some(image(1280, 720, vk::Format::R8_UNORM, 4)),
            display: vk::Extent2D {
                width: 1920,
                height: 1080,
            },
            jitter,
            exposure: 1.25,
            frame_time: 0.016,
            near: 0.05,
            far: f32::INFINITY,
            fov_y: 1.2,
            reset,
        }
    }

    fn dispatch(inputs: &UpscaleInputs) -> ffx::ffxDispatchDescUpscale {
        use ash::vk::Handle;
        let output = image(1920, 1080, vk::Format::R16G16B16A16_SFLOAT, 5);
        let exposure = image(1, 1, vk::Format::R16G16B16A16_SFLOAT, 6);
        dispatch_desc(inputs, vk::CommandBuffer::from_raw(7), &output, &exposure)
    }

    #[test]
    fn the_reset_flag_and_the_jitter_are_passed_exactly_as_given() {
        for (reset, jitter) in [
            (true, Vec2::new(0.3125, -0.4444)),
            (false, Vec2::new(-0.5, 0.1)),
            (true, Vec2::ZERO),
            (false, Vec2::new(0.123_456_7, -0.000_001)),
        ] {
            let d = dispatch(&inputs(reset, jitter));
            assert_eq!(d.reset, reset);
            assert_eq!(d.jitterOffset.x.to_bits(), jitter.x.to_bits());
            assert_eq!(d.jitterOffset.y.to_bits(), jitter.y.to_bits());
        }
    }

    #[test]
    fn the_inputs_map_to_the_dispatch_descriptor() {
        let d = dispatch(&inputs(false, Vec2::ZERO));
        assert_eq!(
            d.header.type_,
            u64::from(ffx::FFX_API_DISPATCH_DESC_TYPE_UPSCALE)
        );
        assert!(d.header.pNext.is_null());
        assert_eq!(d.commandList as usize, 7);
        assert_eq!(d.color.resource as usize, 1);
        assert_eq!(d.depth.resource as usize, 2);
        assert_eq!(d.motionVectors.resource as usize, 3);
        assert_eq!(d.reactive.resource as usize, 4);
        assert_eq!(d.output.resource as usize, 5);
        assert_eq!(d.exposure.resource as usize, 6);
        assert!(d.transparencyAndComposition.resource.is_null());
        // Formats, usages and states.
        assert_eq!(
            d.color.description.format,
            ffx::FFX_API_SURFACE_FORMAT_R16G16B16A16_FLOAT
        );
        assert_eq!(
            d.depth.description.format,
            ffx::FFX_API_SURFACE_FORMAT_R32_FLOAT
        );
        assert_eq!(
            d.depth.description.usage,
            ffx::FFX_API_RESOURCE_USAGE_DEPTHTARGET
        );
        assert_eq!(
            d.motionVectors.description.format,
            ffx::FFX_API_SURFACE_FORMAT_R16G16_FLOAT
        );
        assert_eq!(
            d.reactive.description.format,
            ffx::FFX_API_SURFACE_FORMAT_R8_UNORM
        );
        assert_eq!(d.output.description.usage, ffx::FFX_API_RESOURCE_USAGE_UAV);
        assert_eq!(d.output.state, ffx::FFX_API_RESOURCE_STATE_UNORDERED_ACCESS);
        assert_eq!(d.color.state, ffx::FFX_API_RESOURCE_STATE_COMPUTE_READ);
        // SAFETY: plain u32 unions, written by `texture`.
        let (w, h) = unsafe {
            (
                d.output.description.__bindgen_anon_1.width,
                d.output.description.__bindgen_anon_2.height,
            )
        };
        assert_eq!((w, h), (1920, 1080));
        // Sizes, motion vector scale and camera.
        assert_eq!((d.renderSize.width, d.renderSize.height), (1280, 720));
        assert_eq!((d.upscaleSize.width, d.upscaleSize.height), (1920, 1080));
        assert_eq!(
            (d.motionVectorScale.x, d.motionVectorScale.y),
            (-1280.0, -720.0)
        );
        assert!((d.frameTimeDelta - 16.0).abs() < 1e-4);
        assert_eq!(d.preExposure, 1.0);
        assert_eq!(d.cameraNear, 0.05);
        assert_eq!(d.cameraFar, f32::MAX);
        assert_eq!(d.cameraFovAngleVertical, 1.2);
        assert_eq!(d.viewSpaceToMetersFactor, 1.0);
        assert!(d.enableSharpening);
        assert_eq!(d.sharpness, SHARPNESS);
    }

    #[test]
    fn a_missing_reactive_mask_is_a_null_resource() {
        let mut i = inputs(false, Vec2::ZERO);
        i.reactive = None;
        assert!(dispatch(&i).reactive.resource.is_null());
    }

    #[test]
    fn the_context_reads_reverse_z_infinite_hdr() {
        let flags = create_flags();
        assert_ne!(flags & ffx::FFX_UPSCALE_ENABLE_DEPTH_INVERTED, 0);
        assert_ne!(flags & ffx::FFX_UPSCALE_ENABLE_DEPTH_INFINITE, 0);
        assert_ne!(flags & ffx::FFX_UPSCALE_ENABLE_HIGH_DYNAMIC_RANGE, 0);
        assert_eq!(flags & ffx::FFX_UPSCALE_ENABLE_AUTO_EXPOSURE, 0);
        assert_eq!(
            flags & ffx::FFX_UPSCALE_ENABLE_MOTION_VECTORS_JITTER_CANCELLATION,
            0
        );
    }

    #[test]
    fn the_create_chain_ends_with_the_vulkan_backend() {
        use ash::vk::Handle;
        unsafe extern "system" fn no_proc(
            _: vk::Device,
            _: *const std::ffi::c_char,
        ) -> vk::PFN_vkVoidFunction {
            None
        }
        let device = FfxDevice {
            device: vk::Device::from_raw(8),
            physical: vk::PhysicalDevice::from_raw(9),
            get_device_proc_addr: no_proc,
        };
        let mut descs = CreateDescs::new(
            &device,
            Extent2D {
                width: 1920,
                height: 1080,
            },
        );
        let head = descs.chain();
        assert_eq!(head, &mut descs.upscale.header as *mut _);
        assert_eq!(
            descs.upscale.header.pNext,
            &mut descs.backend.header as *mut _
        );
        assert!(descs.backend.header.pNext.is_null());
        assert_eq!(
            descs.backend.header.type_,
            u64::from(ffx::FFX_API_CREATE_CONTEXT_DESC_TYPE_BACKEND_VK)
        );
        assert_eq!(descs.backend.vkDevice.as_raw(), 8);
        assert_eq!(descs.upscale.maxRenderSize.width, 1920);
        assert_eq!(descs.upscale.maxUpscaleSize.height, 1080);
    }

    #[test]
    fn the_jitter_sequence_follows_the_library() {
        // 8 * ratio^2, truncated.
        assert_eq!(jitter_phase_count(1920, 1920), 8);
        assert_eq!(jitter_phase_count(1280, 1920), 18);
        assert_eq!(jitter_phase_count(960, 1920), 32);
        assert_eq!(jitter_phase_count(640, 1920), 72);
        assert!(jitter_offset(0, 8).abs_diff_eq(Vec2::new(0.0, -1.0 / 6.0), 1e-6));
        assert_eq!(jitter_offset(8, 8), jitter_offset(0, 8));
        for i in 0..72 {
            let j = jitter_offset(i, 72);
            assert!(j.x.abs() <= 0.5 && j.y.abs() <= 0.5, "{i}: {j}");
        }
    }

    #[test]
    fn surface_formats_follow_the_vulkan_header() {
        assert_eq!(
            surface_format(vk::Format::D32_SFLOAT),
            ffx::FFX_API_SURFACE_FORMAT_R32_FLOAT
        );
        assert_eq!(
            surface_format(vk::Format::B10G11R11_UFLOAT_PACK32),
            ffx::FFX_API_SURFACE_FORMAT_R11G11B10_FLOAT
        );
        assert_eq!(
            surface_format(vk::Format::BC1_RGB_UNORM_BLOCK),
            ffx::FFX_API_SURFACE_FORMAT_UNKNOWN
        );
    }
}
