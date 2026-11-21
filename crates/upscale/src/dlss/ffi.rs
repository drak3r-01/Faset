//! [`Ngx`] over NVIDIA's NGX SDK (Windows x86_64 only): bindings generated
//! by `build.rs` with bindgen from the SDK headers it fetched, and the NGX
//! static library linked in. The NGX static library finds the NGX runtime of
//! the NVIDIA driver at initialisation, so a machine without one only gets a
//! failing `init`.

use std::ffi::{c_void, CStr};
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr::{self, NonNull};
use std::sync::{Mutex, PoisonError};

use ash::vk::{self, Handle};

use super::mapping::{flags, names, perf, PROJECT_ID};
use super::ngx::{Capability, Feature, Ngx, NgxDevice, NgxImage, NgxResult, Param, Value};

#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    clippy::all
)]
mod sys {
    include!(concat!(env!("OUT_DIR"), "/ngx_bindings.rs"));
}

// The names and values of `mapping.rs`, checked against the SDK headers.

const fn same(ours: &CStr, sdk: &[u8]) -> bool {
    let ours = ours.to_bytes_with_nul();
    if ours.len() != sdk.len() {
        return false;
    }
    let mut i = 0;
    while i < ours.len() {
        if ours[i] != sdk[i] {
            return false;
        }
        i += 1;
    }
    true
}

macro_rules! same_names {
    ($($ours:ident = $sdk:ident),* $(,)?) => {
        $(const _: () = assert!(
            same(names::$ours, sys::$sdk),
            concat!("parameter ", stringify!($ours), " differs from ", stringify!($sdk)),
        );)*
    };
}

same_names! {
    WIDTH = NVSDK_NGX_Parameter_Width,
    HEIGHT = NVSDK_NGX_Parameter_Height,
    OUT_WIDTH = NVSDK_NGX_Parameter_OutWidth,
    OUT_HEIGHT = NVSDK_NGX_Parameter_OutHeight,
    PERF_QUALITY_VALUE = NVSDK_NGX_Parameter_PerfQualityValue,
    CREATE_FLAGS = NVSDK_NGX_Parameter_DLSS_Feature_Create_Flags,
    ENABLE_OUTPUT_SUBRECTS = NVSDK_NGX_Parameter_DLSS_Enable_Output_Subrects,
    CREATION_NODE_MASK = NVSDK_NGX_Parameter_CreationNodeMask,
    VISIBILITY_NODE_MASK = NVSDK_NGX_Parameter_VisibilityNodeMask,
    COLOR = NVSDK_NGX_Parameter_Color,
    OUTPUT = NVSDK_NGX_Parameter_Output,
    DEPTH = NVSDK_NGX_Parameter_Depth,
    MOTION_VECTORS = NVSDK_NGX_Parameter_MotionVectors,
    EXPOSURE_TEXTURE = NVSDK_NGX_Parameter_ExposureTexture,
    BIAS_CURRENT_COLOR_MASK = NVSDK_NGX_Parameter_DLSS_Input_Bias_Current_Color_Mask,
    TRANSPARENCY_MASK = NVSDK_NGX_Parameter_TransparencyMask,
    JITTER_OFFSET_X = NVSDK_NGX_Parameter_Jitter_Offset_X,
    JITTER_OFFSET_Y = NVSDK_NGX_Parameter_Jitter_Offset_Y,
    RESET = NVSDK_NGX_Parameter_Reset,
    MV_SCALE_X = NVSDK_NGX_Parameter_MV_Scale_X,
    MV_SCALE_Y = NVSDK_NGX_Parameter_MV_Scale_Y,
    RENDER_SUBRECT_WIDTH = NVSDK_NGX_Parameter_DLSS_Render_Subrect_Dimensions_Width,
    RENDER_SUBRECT_HEIGHT = NVSDK_NGX_Parameter_DLSS_Render_Subrect_Dimensions_Height,
    PRE_EXPOSURE = NVSDK_NGX_Parameter_DLSS_Pre_Exposure,
    EXPOSURE_SCALE = NVSDK_NGX_Parameter_DLSS_Exposure_Scale,
    FRAME_TIME_MS = NVSDK_NGX_Parameter_FrameTimeDeltaInMsec,
    SHARPNESS = NVSDK_NGX_Parameter_Sharpness,
    SS_AVAILABLE = NVSDK_NGX_Parameter_SuperSampling_Available,
    SS_NEEDS_UPDATED_DRIVER = NVSDK_NGX_Parameter_SuperSampling_NeedsUpdatedDriver,
    SS_MIN_DRIVER_MAJOR = NVSDK_NGX_Parameter_SuperSampling_MinDriverVersionMajor,
    SS_MIN_DRIVER_MINOR = NVSDK_NGX_Parameter_SuperSampling_MinDriverVersionMinor,
    SS_FEATURE_INIT_RESULT = NVSDK_NGX_Parameter_SuperSampling_FeatureInitResult,
}

// The casts state the type compared, whatever integer type bindgen picks.
#[allow(clippy::unnecessary_cast)]
const _: () = {
    assert!(perf::MAX_PERF == sys::NVSDK_NGX_PerfQuality_Value_MaxPerf as i32);
    assert!(perf::BALANCED == sys::NVSDK_NGX_PerfQuality_Value_Balanced as i32);
    assert!(perf::MAX_QUALITY == sys::NVSDK_NGX_PerfQuality_Value_MaxQuality as i32);
    assert!(perf::ULTRA_PERFORMANCE == sys::NVSDK_NGX_PerfQuality_Value_UltraPerformance as i32);
    assert!(perf::DLAA == sys::NVSDK_NGX_PerfQuality_Value_DLAA as i32);
    assert!(flags::IS_HDR == sys::NVSDK_NGX_DLSS_Feature_Flags_IsHDR as i32);
    assert!(flags::MV_LOW_RES == sys::NVSDK_NGX_DLSS_Feature_Flags_MVLowRes as i32);
    assert!(flags::MV_JITTERED == sys::NVSDK_NGX_DLSS_Feature_Flags_MVJittered as i32);
    assert!(flags::DEPTH_INVERTED == sys::NVSDK_NGX_DLSS_Feature_Flags_DepthInverted as i32);
    assert!(flags::AUTO_EXPOSURE == sys::NVSDK_NGX_DLSS_Feature_Flags_AutoExposure as i32);
    assert!(NgxResult::SUCCESS.0 == sys::NVSDK_NGX_Result_Success as u32);
    assert!(
        NgxResult::FAIL_FEATURE_NOT_SUPPORTED.0
            == sys::NVSDK_NGX_Result_FAIL_FeatureNotSupported as u32
    );
    assert!(NgxResult::FAIL_OUT_OF_DATE.0 == sys::NVSDK_NGX_Result_FAIL_OutOfDate as u32);
};

/// The engine version given to NGX.
const ENGINE_VERSION: &CStr =
    match CStr::from_bytes_with_nul(concat!(env!("CARGO_PKG_VERSION"), "\0").as_bytes()) {
        Ok(v) => v,
        Err(_) => panic!("the package version has a null byte"),
    };

fn result(code: sys::NVSDK_NGX_Result) -> NgxResult {
    NgxResult(code as u32)
}

/// NGX keeps one instance per device: the live `init`s of each device, so
/// that a probe never shuts down the instance an upscaler is using.
static SESSIONS: Mutex<Vec<(u64, u32)>> = Mutex::new(Vec::new());

/// The NGX SDK linked into this build.
pub(crate) struct SystemNgx;

fn resource(image: &NgxImage) -> sys::NVSDK_NGX_Resource_VK {
    sys::NVSDK_NGX_Resource_VK {
        Resource: sys::NVSDK_NGX_Resource_VK__bindgen_ty_1 {
            ImageViewInfo: sys::NVSDK_NGX_ImageViewInfo_VK {
                ImageView: image.view.as_raw(),
                Image: image.image.as_raw(),
                SubresourceRange: sys::VkImageSubresourceRange {
                    aspectMask: image.aspect.as_raw(),
                    baseMipLevel: 0,
                    levelCount: 1,
                    baseArrayLayer: 0,
                    layerCount: 1,
                },
                Format: image.format.as_raw(),
                Width: image.extent.width,
                Height: image.extent.height,
            },
        },
        Type: sys::NVSDK_NGX_RESOURCE_VK_TYPE_VK_IMAGEVIEW,
        ReadWrite: image.read_write,
    }
}

/// Sets `list` on a parameter map. The returned resources are what the image
/// parameters point at: keep them until NGX has read them.
///
/// # Safety
/// `params` must be a live parameter map.
unsafe fn set_all(
    params: *mut sys::NVSDK_NGX_Parameter,
    list: &[Param],
) -> Vec<sys::NVSDK_NGX_Resource_VK> {
    let resources: Vec<_> = list
        .iter()
        .filter_map(|p| match p.value {
            Value::Image(Some(image)) => Some(resource(&image)),
            _ => None,
        })
        .collect();
    let mut next = resources.iter();
    for p in list {
        let name = p.name.as_ptr();
        // SAFETY: a live map and null-terminated names; image pointers stay
        // valid while `resources` lives (it is not modified any more).
        unsafe {
            match p.value {
                Value::Int(v) => sys::NVSDK_NGX_Parameter_SetI(params, name, v),
                Value::Uint(v) => sys::NVSDK_NGX_Parameter_SetUI(params, name, v),
                Value::Float(v) => sys::NVSDK_NGX_Parameter_SetF(params, name, v),
                Value::Image(Some(_)) => {
                    let r = next.next().expect("one resource per image parameter");
                    let r = ptr::from_ref(r).cast_mut().cast::<c_void>();
                    sys::NVSDK_NGX_Parameter_SetVoidPointer(params, name, r);
                }
                Value::Image(None) => {
                    sys::NVSDK_NGX_Parameter_SetVoidPointer(params, name, ptr::null_mut());
                }
            }
        }
    }
    resources
}

impl Ngx for SystemNgx {
    unsafe fn init(&self, device: &NgxDevice, data_path: &Path) -> NgxResult {
        let mut sessions = SESSIONS.lock().unwrap_or_else(PoisonError::into_inner);
        let key = device.device.as_raw();
        if let Some((_, count)) = sessions.iter_mut().find(|(k, _)| *k == key) {
            *count += 1;
            return NgxResult::SUCCESS;
        }
        let path: Vec<u16> = data_path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: live handles from the caller; null-terminated strings that
        // outlive the call; no feature info (DLLs next to the executable). The
        // instance-level entry point is optional (NGX loads its own).
        let code = result(unsafe {
            sys::NVSDK_NGX_VULKAN_Init_with_ProjectID(
                PROJECT_ID.as_ptr(),
                sys::NVSDK_NGX_ENGINE_TYPE_CUSTOM,
                ENGINE_VERSION.as_ptr(),
                path.as_ptr(),
                device.instance,
                device.physical,
                device.device,
                None,
                Some(device.get_device_proc_addr),
                ptr::null(),
                sys::NVSDK_NGX_Version_API,
            )
        });
        if !code.failed() {
            sessions.push((key, 1));
        }
        code
    }

    unsafe fn shutdown(&self, device: &NgxDevice) -> NgxResult {
        let mut sessions = SESSIONS.lock().unwrap_or_else(PoisonError::into_inner);
        let key = device.device.as_raw();
        let Some(i) = sessions.iter().position(|(k, _)| *k == key) else {
            return NgxResult::FAIL_NOT_INITIALIZED;
        };
        sessions[i].1 -= 1;
        if sessions[i].1 > 0 {
            return NgxResult::SUCCESS;
        }
        sessions.remove(i);
        // SAFETY: NGX is initialised on this live device and nothing uses it
        // any more (last session).
        result(unsafe { sys::NVSDK_NGX_VULKAN_Shutdown1(device.device) })
    }

    unsafe fn capability(&self) -> Result<Capability, NgxResult> {
        let mut params: *mut sys::NVSDK_NGX_Parameter = ptr::null_mut();
        // SAFETY: NGX is initialised (caller).
        result(unsafe { sys::NVSDK_NGX_VULKAN_GetCapabilityParameters(&mut params) }).check()?;
        if params.is_null() {
            return Err(NgxResult::FAIL_NOT_INITIALIZED);
        }
        let int = |name: &CStr| {
            let mut v = 0;
            // SAFETY: a live map; a missing entry leaves `v` at 0.
            unsafe { sys::NVSDK_NGX_Parameter_GetI(params, name.as_ptr(), &mut v) };
            v
        };
        let uint = |name: &CStr| {
            let mut v = 0;
            // SAFETY: as above.
            unsafe { sys::NVSDK_NGX_Parameter_GetUI(params, name.as_ptr(), &mut v) };
            v
        };
        let cap = Capability {
            available: int(names::SS_AVAILABLE) != 0,
            needs_updated_driver: int(names::SS_NEEDS_UPDATED_DRIVER) != 0,
            min_driver: (
                uint(names::SS_MIN_DRIVER_MAJOR),
                uint(names::SS_MIN_DRIVER_MINOR),
            ),
            init_result: NgxResult(int(names::SS_FEATURE_INIT_RESULT) as u32),
        };
        // SAFETY: the map came from GetCapabilityParameters and is not used
        // any more.
        unsafe { sys::NVSDK_NGX_VULKAN_DestroyParameters(params) };
        Ok(cap)
    }

    unsafe fn create_feature(
        &self,
        device: &NgxDevice,
        cmd: vk::CommandBuffer,
        list: &[Param],
    ) -> Result<Feature, NgxResult> {
        let mut params: *mut sys::NVSDK_NGX_Parameter = ptr::null_mut();
        // SAFETY: NGX is initialised (caller).
        result(unsafe { sys::NVSDK_NGX_VULKAN_AllocateParameters(&mut params) }).check()?;
        let Some(params) = NonNull::new(params) else {
            return Err(NgxResult::FAIL_NOT_INITIALIZED);
        };
        let mut handle: *mut sys::NVSDK_NGX_Handle = ptr::null_mut();
        // SAFETY: a live map; `cmd` is recording on the device (caller).
        let code = unsafe {
            let _resources = set_all(params.as_ptr(), list);
            result(sys::NVSDK_NGX_VULKAN_CreateFeature1(
                device.device,
                cmd,
                sys::NVSDK_NGX_Feature_SuperSampling,
                params.as_ptr(),
                &mut handle,
            ))
        };
        match (code.check(), NonNull::new(handle)) {
            (Ok(()), Some(handle)) => Ok(Feature {
                handle: handle.cast(),
                params: params.cast(),
            }),
            (result, _) => {
                // SAFETY: the map is ours and unused.
                unsafe { sys::NVSDK_NGX_VULKAN_DestroyParameters(params.as_ptr()) };
                Err(result.err().unwrap_or(NgxResult::FAIL_NOT_INITIALIZED))
            }
        }
    }

    unsafe fn evaluate(
        &self,
        cmd: vk::CommandBuffer,
        feature: &Feature,
        list: &[Param],
    ) -> NgxResult {
        let params = feature.params.as_ptr().cast::<sys::NVSDK_NGX_Parameter>();
        // SAFETY: a live feature and map; the resources outlive the call; the
        // images are live and in the right layouts (caller).
        unsafe {
            let _resources = set_all(params, list);
            result(sys::NVSDK_NGX_VULKAN_EvaluateFeature_C(
                cmd,
                feature.handle.as_ptr().cast(),
                params,
                None,
            ))
        }
    }

    unsafe fn release(&self, feature: Feature) -> NgxResult {
        // SAFETY: a live feature and map, released once (consumed); the GPU
        // is done with them (caller).
        unsafe {
            let code = result(sys::NVSDK_NGX_VULKAN_ReleaseFeature(
                feature.handle.as_ptr().cast(),
            ));
            sys::NVSDK_NGX_VULKAN_DestroyParameters(feature.params.as_ptr().cast());
            code
        }
    }
}
