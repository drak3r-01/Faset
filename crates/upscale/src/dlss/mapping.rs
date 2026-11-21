//! From the renderer's [`UpscaleInputs`] to the NGX parameters of DLSS:
//! feature creation, per-frame evaluation, quality levels. Plain data, no
//! call into NGX. On Windows x86_64 the names and values below are checked
//! at compile time against the SDK headers (`ffi.rs`).

use ash::vk;
use faset_render::{ImageHandle, UpscaleInputs};

use super::ngx::{NgxImage, Param, Value};
use crate::{Extent2D, Quality};

// The names, values and project ID are also read by `ffi.rs`, which exists
// on Windows x86_64 only; elsewhere some are only read by the tests.

/// `NVSDK_NGX_Parameter_*` names used here.
#[allow(dead_code)]
pub(crate) mod names {
    use std::ffi::CStr;

    pub const WIDTH: &CStr = c"Width";
    pub const HEIGHT: &CStr = c"Height";
    pub const OUT_WIDTH: &CStr = c"OutWidth";
    pub const OUT_HEIGHT: &CStr = c"OutHeight";
    pub const PERF_QUALITY_VALUE: &CStr = c"PerfQualityValue";
    pub const CREATE_FLAGS: &CStr = c"DLSS.Feature.Create.Flags";
    pub const ENABLE_OUTPUT_SUBRECTS: &CStr = c"DLSS.Enable.Output.Subrects";
    pub const CREATION_NODE_MASK: &CStr = c"CreationNodeMask";
    pub const VISIBILITY_NODE_MASK: &CStr = c"VisibilityNodeMask";
    pub const COLOR: &CStr = c"Color";
    pub const OUTPUT: &CStr = c"Output";
    pub const DEPTH: &CStr = c"Depth";
    pub const MOTION_VECTORS: &CStr = c"MotionVectors";
    pub const EXPOSURE_TEXTURE: &CStr = c"ExposureTexture";
    pub const BIAS_CURRENT_COLOR_MASK: &CStr = c"DLSS.Input.Bias.Current.Color.Mask";
    pub const TRANSPARENCY_MASK: &CStr = c"TransparencyMask";
    pub const JITTER_OFFSET_X: &CStr = c"Jitter.Offset.X";
    pub const JITTER_OFFSET_Y: &CStr = c"Jitter.Offset.Y";
    pub const RESET: &CStr = c"Reset";
    pub const MV_SCALE_X: &CStr = c"MV.Scale.X";
    pub const MV_SCALE_Y: &CStr = c"MV.Scale.Y";
    pub const RENDER_SUBRECT_WIDTH: &CStr = c"DLSS.Render.Subrect.Dimensions.Width";
    pub const RENDER_SUBRECT_HEIGHT: &CStr = c"DLSS.Render.Subrect.Dimensions.Height";
    pub const PRE_EXPOSURE: &CStr = c"DLSS.Pre.Exposure";
    pub const EXPOSURE_SCALE: &CStr = c"DLSS.Exposure.Scale";
    pub const FRAME_TIME_MS: &CStr = c"FrameTimeDeltaInMsec";
    pub const SHARPNESS: &CStr = c"Sharpness";
    pub const SS_AVAILABLE: &CStr = c"SuperSampling.Available";
    pub const SS_NEEDS_UPDATED_DRIVER: &CStr = c"SuperSampling.NeedsUpdatedDriver";
    pub const SS_MIN_DRIVER_MAJOR: &CStr = c"SuperSampling.MinDriverVersionMajor";
    pub const SS_MIN_DRIVER_MINOR: &CStr = c"SuperSampling.MinDriverVersionMinor";
    pub const SS_FEATURE_INIT_RESULT: &CStr = c"SuperSampling.FeatureInitResult";
}

/// `NVSDK_NGX_PerfQuality_Value`.
#[allow(dead_code)]
pub(crate) mod perf {
    pub const MAX_PERF: i32 = 0;
    pub const BALANCED: i32 = 1;
    pub const MAX_QUALITY: i32 = 2;
    pub const ULTRA_PERFORMANCE: i32 = 3;
    pub const DLAA: i32 = 5;
}

/// `NVSDK_NGX_DLSS_Feature_Flags`.
#[allow(dead_code)]
pub(crate) mod flags {
    pub const IS_HDR: i32 = 1 << 0;
    pub const MV_LOW_RES: i32 = 1 << 1;
    pub const MV_JITTERED: i32 = 1 << 2;
    pub const DEPTH_INVERTED: i32 = 1 << 3;
    pub const AUTO_EXPOSURE: i32 = 1 << 6;
}

/// Decision: the GUID-like project ID NGX asks of custom engines
/// (`NVSDK_NGX_VULKAN_Init_with_ProjectID`), fixed for Faset.
#[allow(dead_code)]
pub(crate) const PROJECT_ID: &std::ffi::CStr = c"3b9e7c1a-5d2f-4e8b-9c4a-6f1d2e3a4b5c";

/// The Vulkan device extensions NGX needs for DLSS, each with a command it
/// provides: `vkGetDeviceProcAddr` returns null for the commands of an
/// extension the device was created without.
pub(crate) const DEVICE_EXTENSIONS: [(&str, &std::ffi::CStr); 3] = [
    ("VK_NVX_binary_import", c"vkCreateCuModuleNVX"),
    ("VK_NVX_image_view_handle", c"vkGetImageViewHandleNVX"),
    ("VK_KHR_push_descriptor", c"vkCmdPushDescriptorSetKHR"),
];

/// The DLSS mode of a quality level ([`crate::effective_quality`] already
/// turned a free scale into `Native`, which is DLAA).
pub(crate) fn perf_quality(quality: Quality) -> i32 {
    match quality {
        Quality::Quality => perf::MAX_QUALITY,
        Quality::Balanced => perf::BALANCED,
        Quality::Performance => perf::MAX_PERF,
        Quality::UltraPerformance => perf::ULTRA_PERFORMANCE,
        Quality::Native | Quality::Free(_) => perf::DLAA,
    }
}

/// Creation flags: linear HDR colour before tonemapping, motion vectors at
/// the render resolution computed from the unjittered matrices (so not
/// `MV_JITTERED`), and the reverse-Z depth of lot 2.2 (`z_ndc = n / -z_view`:
/// near = 1, infinity = 0). The exposure is given, not computed.
pub(crate) fn create_flags() -> i32 {
    flags::IS_HDR | flags::MV_LOW_RES | flags::DEPTH_INVERTED
}

fn p(name: &'static std::ffi::CStr, value: Value) -> Param {
    Param { name, value }
}

/// Parameters of feature creation for one (render, display, quality).
pub(crate) fn create_params(render: Extent2D, display: Extent2D, quality: Quality) -> Vec<Param> {
    use names::*;
    vec![
        p(CREATION_NODE_MASK, Value::Uint(1)),
        p(VISIBILITY_NODE_MASK, Value::Uint(1)),
        p(WIDTH, Value::Uint(render.width)),
        p(HEIGHT, Value::Uint(render.height)),
        p(OUT_WIDTH, Value::Uint(display.width)),
        p(OUT_HEIGHT, Value::Uint(display.height)),
        p(PERF_QUALITY_VALUE, Value::Int(perf_quality(quality))),
        p(CREATE_FLAGS, Value::Int(create_flags())),
        p(ENABLE_OUTPUT_SUBRECTS, Value::Int(0)),
    ]
}

fn image(handle: &ImageHandle, aspect: vk::ImageAspectFlags, read_write: bool) -> NgxImage {
    NgxImage {
        image: handle.image,
        view: handle.view,
        format: handle.format,
        extent: handle.extent,
        aspect,
        read_write,
    }
}

/// Parameters of one evaluation.
///
/// * `output`: display-sized storage image, in `GENERAL`.
/// * `exposure`: a 1 x 1 image holding `inputs.exposure` in its red channel;
///   the colour is not pre-exposed (pre-exposure 1).
/// * The reactive mask (weather particles) biases DLSS towards the current
///   colour; without one the slot is cleared.
/// * The motion vectors are `current - previous` in UV units; DLSS wants
///   `previous - current` in render pixels, hence the negative scale of the
///   render size.
/// * Frame time in milliseconds. Sharpening is deprecated in DLSS and off.
/// * The jitter and the reset flag are passed exactly as given.
/// * DLSS (super sampling) takes no near/far plane or field of view: the
///   depth layout is given by `DEPTH_INVERTED` at creation.
pub(crate) fn evaluate_params(
    inputs: &UpscaleInputs,
    output: &ImageHandle,
    exposure: &ImageHandle,
) -> Vec<Param> {
    use names::*;
    let colour = vk::ImageAspectFlags::COLOR;
    let render = inputs.colour.extent;
    vec![
        p(
            COLOR,
            Value::Image(Some(image(&inputs.colour, colour, false))),
        ),
        p(OUTPUT, Value::Image(Some(image(output, colour, true)))),
        p(
            DEPTH,
            Value::Image(Some(image(
                &inputs.depth,
                vk::ImageAspectFlags::DEPTH,
                false,
            ))),
        ),
        p(
            MOTION_VECTORS,
            Value::Image(Some(image(&inputs.motion, colour, false))),
        ),
        p(
            EXPOSURE_TEXTURE,
            Value::Image(Some(image(exposure, colour, false))),
        ),
        p(
            BIAS_CURRENT_COLOR_MASK,
            Value::Image(inputs.reactive.as_ref().map(|r| image(r, colour, false))),
        ),
        p(TRANSPARENCY_MASK, Value::Image(None)),
        p(JITTER_OFFSET_X, Value::Float(inputs.jitter.x)),
        p(JITTER_OFFSET_Y, Value::Float(inputs.jitter.y)),
        p(RESET, Value::Int(i32::from(inputs.reset))),
        p(MV_SCALE_X, Value::Float(-(render.width as f32))),
        p(MV_SCALE_Y, Value::Float(-(render.height as f32))),
        p(RENDER_SUBRECT_WIDTH, Value::Uint(render.width)),
        p(RENDER_SUBRECT_HEIGHT, Value::Uint(render.height)),
        p(PRE_EXPOSURE, Value::Float(1.0)),
        p(EXPOSURE_SCALE, Value::Float(1.0)),
        p(FRAME_TIME_MS, Value::Float(inputs.frame_time * 1000.0)),
        p(SHARPNESS, Value::Float(0.0)),
    ]
}

#[cfg(test)]
mod tests {
    use std::ffi::CStr;

    use faset_render::glam::Vec2;

    use super::*;

    fn handle(width: u32, height: u32, format: vk::Format, raw: u64) -> ImageHandle {
        use ash::vk::Handle;
        ImageHandle {
            image: vk::Image::from_raw(raw),
            view: vk::ImageView::from_raw(raw + 100),
            extent: vk::Extent2D { width, height },
            format,
        }
    }

    fn inputs(reset: bool, jitter: Vec2) -> UpscaleInputs {
        UpscaleInputs {
            colour: handle(1280, 720, vk::Format::R16G16B16A16_SFLOAT, 1),
            depth: handle(1280, 720, vk::Format::D32_SFLOAT, 2),
            motion: handle(1280, 720, vk::Format::R16G16_SFLOAT, 3),
            reactive: Some(handle(1280, 720, vk::Format::R8_UNORM, 4)),
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

    fn evaluate(inputs: &UpscaleInputs) -> Vec<Param> {
        let output = handle(1920, 1080, vk::Format::R16G16B16A16_SFLOAT, 5);
        let exposure = handle(1, 1, vk::Format::R16G16B16A16_SFLOAT, 6);
        evaluate_params(inputs, &output, &exposure)
    }

    fn get(params: &[Param], name: &CStr) -> Value {
        let found: Vec<_> = params.iter().filter(|p| p.name == name).collect();
        assert_eq!(found.len(), 1, "{name:?} set {} times", found.len());
        found[0].value
    }

    fn img(params: &[Param], name: &CStr) -> Option<NgxImage> {
        match get(params, name) {
            Value::Image(i) => i,
            other => panic!("{name:?} is {other:?}"),
        }
    }

    fn float_bits(params: &[Param], name: &CStr) -> u32 {
        match get(params, name) {
            Value::Float(f) => f.to_bits(),
            other => panic!("{name:?} is {other:?}"),
        }
    }

    #[test]
    fn the_reset_flag_and_the_jitter_are_passed_exactly_as_given() {
        for (reset, jitter) in [
            (true, Vec2::new(0.3125, -0.4444)),
            (false, Vec2::new(-0.5, 0.1)),
            (true, Vec2::ZERO),
            (false, Vec2::new(0.123_456_7, -0.000_001)),
            (true, Vec2::new(-0.0, 0.499_999_97)),
        ] {
            let params = evaluate(&inputs(reset, jitter));
            assert_eq!(
                get(&params, names::RESET),
                Value::Int(i32::from(reset)),
                "{reset}"
            );
            assert_eq!(
                float_bits(&params, names::JITTER_OFFSET_X),
                jitter.x.to_bits()
            );
            assert_eq!(
                float_bits(&params, names::JITTER_OFFSET_Y),
                jitter.y.to_bits()
            );
        }
    }

    #[test]
    fn the_inputs_map_to_the_evaluation_parameters() {
        use ash::vk::Handle;
        let params = evaluate(&inputs(false, Vec2::ZERO));
        let colour = img(&params, names::COLOR).unwrap();
        assert_eq!(colour.image.as_raw(), 1);
        assert_eq!(colour.view.as_raw(), 101);
        assert_eq!(colour.format, vk::Format::R16G16B16A16_SFLOAT);
        assert_eq!(colour.aspect, vk::ImageAspectFlags::COLOR);
        assert!(!colour.read_write);
        let depth = img(&params, names::DEPTH).unwrap();
        assert_eq!(depth.image.as_raw(), 2);
        assert_eq!(depth.format, vk::Format::D32_SFLOAT);
        assert_eq!(depth.aspect, vk::ImageAspectFlags::DEPTH);
        assert_eq!(
            img(&params, names::MOTION_VECTORS).unwrap().image.as_raw(),
            3
        );
        assert_eq!(
            img(&params, names::BIAS_CURRENT_COLOR_MASK)
                .unwrap()
                .image
                .as_raw(),
            4
        );
        let output = img(&params, names::OUTPUT).unwrap();
        assert_eq!(output.image.as_raw(), 5);
        assert!(output.read_write, "the output is written");
        assert_eq!(
            output.extent,
            vk::Extent2D {
                width: 1920,
                height: 1080
            }
        );
        assert_eq!(
            img(&params, names::EXPOSURE_TEXTURE)
                .unwrap()
                .image
                .as_raw(),
            6
        );
        assert_eq!(img(&params, names::TRANSPARENCY_MASK), None);
        // Motion vector scale, render size, exposure and frame time.
        assert_eq!(get(&params, names::MV_SCALE_X), Value::Float(-1280.0));
        assert_eq!(get(&params, names::MV_SCALE_Y), Value::Float(-720.0));
        assert_eq!(get(&params, names::RENDER_SUBRECT_WIDTH), Value::Uint(1280));
        assert_eq!(get(&params, names::RENDER_SUBRECT_HEIGHT), Value::Uint(720));
        assert_eq!(get(&params, names::PRE_EXPOSURE), Value::Float(1.0));
        assert_eq!(get(&params, names::EXPOSURE_SCALE), Value::Float(1.0));
        match get(&params, names::FRAME_TIME_MS) {
            Value::Float(ms) => assert!((ms - 16.0).abs() < 1e-4, "{ms}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(get(&params, names::SHARPNESS), Value::Float(0.0));
    }

    #[test]
    fn a_missing_reactive_mask_clears_its_slot() {
        let mut i = inputs(false, Vec2::ZERO);
        i.reactive = None;
        assert_eq!(img(&evaluate(&i), names::BIAS_CURRENT_COLOR_MASK), None);
    }

    #[test]
    fn creation_reads_reverse_z_hdr_and_low_res_unjittered_motion() {
        let f = create_flags();
        assert_ne!(f & flags::IS_HDR, 0);
        assert_ne!(f & flags::DEPTH_INVERTED, 0);
        assert_ne!(f & flags::MV_LOW_RES, 0);
        assert_eq!(f & flags::MV_JITTERED, 0);
        assert_eq!(f & flags::AUTO_EXPOSURE, 0);

        let render = Extent2D {
            width: 1129,
            height: 635,
        };
        let display = Extent2D {
            width: 1920,
            height: 1080,
        };
        let params = create_params(render, display, Quality::Balanced);
        assert_eq!(get(&params, names::WIDTH), Value::Uint(1129));
        assert_eq!(get(&params, names::HEIGHT), Value::Uint(635));
        assert_eq!(get(&params, names::OUT_WIDTH), Value::Uint(1920));
        assert_eq!(get(&params, names::OUT_HEIGHT), Value::Uint(1080));
        assert_eq!(
            get(&params, names::PERF_QUALITY_VALUE),
            Value::Int(perf::BALANCED)
        );
        assert_eq!(get(&params, names::CREATE_FLAGS), Value::Int(f));
        assert_eq!(get(&params, names::CREATION_NODE_MASK), Value::Uint(1));
        assert_eq!(get(&params, names::ENABLE_OUTPUT_SUBRECTS), Value::Int(0));
    }

    #[test]
    fn quality_levels_map_to_dlss_modes() {
        assert_eq!(perf_quality(Quality::Quality), perf::MAX_QUALITY);
        assert_eq!(perf_quality(Quality::Balanced), perf::BALANCED);
        assert_eq!(perf_quality(Quality::Performance), perf::MAX_PERF);
        assert_eq!(
            perf_quality(Quality::UltraPerformance),
            perf::ULTRA_PERFORMANCE
        );
        assert_eq!(perf_quality(Quality::Native), perf::DLAA);
    }

    #[test]
    fn the_project_id_is_guid_like() {
        let id = PROJECT_ID.to_str().unwrap();
        let groups: Vec<usize> = id.split('-').map(str::len).collect();
        assert_eq!(groups, [8, 4, 4, 4, 12]);
        assert!(id.chars().all(|c| c == '-' || c.is_ascii_hexdigit()));
    }
}
