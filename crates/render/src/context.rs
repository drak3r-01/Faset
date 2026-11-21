//! Vulkan instance, validation messenger, physical/logical device, queue and
//! memory allocator.

use std::ffi::{c_char, c_void, CStr};
use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicU32, Ordering};

use ash::{ext::debug_utils, khr, vk, Device, Entry, Instance};
use gpu_allocator::vulkan::{Allocator, AllocatorCreateDesc};
use raw_window_handle::{RawDisplayHandle, RawWindowHandle};

use crate::select::{self, Candidate, DeviceKind};
use crate::{DeviceExtensions, GpuPreference, RenderError};

const VALIDATION_LAYER: &CStr = c"VK_LAYER_KHRONOS_validation";

/// Raw handles of the window a surface is created for.
pub(crate) struct WindowHandles {
    pub display: RawDisplayHandle,
    pub window: RawWindowHandle,
}

struct Messenger {
    utils: debug_utils::Instance,
    handle: vk::DebugUtilsMessengerEXT,
}

pub(crate) struct Surface {
    pub loader: khr::surface::Instance,
    pub handle: vk::SurfaceKHR,
}

pub(crate) struct Context {
    _entry: Entry,
    pub instance: Instance,
    messenger: Option<Messenger>,
    /// Warning and error messages seen by the messenger. Boxed so the address
    /// handed to Vulkan as user data never moves.
    messages: Box<AtomicU32>,
    pub surface: Option<Surface>,
    pub physical: vk::PhysicalDevice,
    pub device: Device,
    pub queue: vk::Queue,
    pub queue_family: u32,
    pub dynamic_rendering: khr::dynamic_rendering::Device,
    /// Optional extensions the device was created with.
    pub extensions: DeviceExtensions,
    pub allocator: ManuallyDrop<Allocator>,
}

/// Counts and logs warning/error messages from the validation layer.
unsafe extern "system" fn on_debug_message(
    severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    _kind: vk::DebugUtilsMessageTypeFlagsEXT,
    data: *const vk::DebugUtilsMessengerCallbackDataEXT<'_>,
    user: *mut c_void,
) -> vk::Bool32 {
    if !user.is_null() {
        // SAFETY: `user` is the boxed counter owned by `Context`, which outlives
        // the messenger and the instance.
        let counter = unsafe { &*(user as *const AtomicU32) };
        counter.fetch_add(1, Ordering::SeqCst);
    }
    // SAFETY: Vulkan passes a valid callback data pointer for the call duration.
    let message = unsafe { data.as_ref().and_then(|d| d.message_as_c_str()) }
        .map(|m| m.to_string_lossy().into_owned())
        .unwrap_or_default();
    if severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::ERROR) {
        tracing::error!(target: "vulkan", "{message}");
    } else {
        tracing::warn!(target: "vulkan", "{message}");
    }
    vk::FALSE
}

fn has_device_extension(instance: &Instance, physical: vk::PhysicalDevice, name: &CStr) -> bool {
    // SAFETY: `physical` was enumerated from `instance`.
    unsafe { instance.enumerate_device_extension_properties(physical) }
        .map(|exts| exts.iter().any(|e| e.extension_name_as_c_str() == Ok(name)))
        .unwrap_or(false)
}

/// The optional extensions `physical` offers (none when the query fails).
fn optional_extensions(instance: &Instance, physical: vk::PhysicalDevice) -> DeviceExtensions {
    // SAFETY: `physical` was enumerated from `instance`.
    let offered =
        unsafe { instance.enumerate_device_extension_properties(physical) }.unwrap_or_default();
    DeviceExtensions::from_offered(
        offered
            .iter()
            .filter_map(|e| e.extension_name_as_c_str().ok()),
    )
}

impl Context {
    pub(crate) fn new(
        gpu: GpuPreference,
        window: Option<&WindowHandles>,
    ) -> Result<Self, RenderError> {
        // SAFETY: loading the system Vulkan loader; its entry points are only
        // used through `ash`.
        let entry = unsafe { Entry::load() }?;

        // SAFETY: plain global query.
        let version =
            unsafe { entry.try_enumerate_instance_version() }?.unwrap_or(vk::API_VERSION_1_0);
        if version < vk::API_VERSION_1_2 {
            return Err(RenderError::UnsupportedVersion(format!(
                "{}.{}",
                vk::api_version_major(version),
                vk::api_version_minor(version)
            )));
        }

        let validation = cfg!(debug_assertions) && {
            // SAFETY: plain global query.
            let layers = unsafe { entry.enumerate_instance_layer_properties() }?;
            let found = layers
                .iter()
                .any(|l| l.layer_name_as_c_str() == Ok(VALIDATION_LAYER));
            if !found {
                tracing::warn!("Khronos validation layer not installed; running without it");
            }
            found
        };

        let mut extensions: Vec<*const c_char> = Vec::new();
        if let Some(w) = window {
            extensions.extend_from_slice(ash_window::enumerate_required_extensions(w.display)?);
        }
        if validation {
            extensions.push(debug_utils::NAME.as_ptr());
        }
        #[allow(unused_mut)]
        let mut flags = vk::InstanceCreateFlags::empty();
        #[cfg(target_os = "macos")]
        {
            extensions.push(khr::portability_enumeration::NAME.as_ptr());
            flags |= vk::InstanceCreateFlags::ENUMERATE_PORTABILITY_KHR;
        }
        let layers: Vec<*const c_char> = if validation {
            vec![VALIDATION_LAYER.as_ptr()]
        } else {
            Vec::new()
        };

        let messages = Box::new(AtomicU32::new(0));
        let mut messenger_info = vk::DebugUtilsMessengerCreateInfoEXT::default()
            .message_severity(
                vk::DebugUtilsMessageSeverityFlagsEXT::WARNING
                    | vk::DebugUtilsMessageSeverityFlagsEXT::ERROR,
            )
            .message_type(
                vk::DebugUtilsMessageTypeFlagsEXT::GENERAL
                    | vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION
                    | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE,
            )
            .pfn_user_callback(Some(on_debug_message))
            .user_data(std::ptr::from_ref::<AtomicU32>(&messages).cast_mut().cast());

        let app = vk::ApplicationInfo::default()
            .application_name(c"Faset")
            .engine_name(c"Faset")
            .api_version(vk::API_VERSION_1_2);
        let mut instance_info = vk::InstanceCreateInfo::default()
            .application_info(&app)
            .flags(flags)
            .enabled_layer_names(&layers)
            .enabled_extension_names(&extensions);
        if validation {
            // Also reports problems inside vkCreateInstance/vkDestroyInstance.
            instance_info = instance_info.push_next(&mut messenger_info);
        }
        // SAFETY: every pointer in `instance_info` outlives the call.
        let instance = unsafe { entry.create_instance(&instance_info, None) }?;

        // From here on, partially built state is torn down by `Partial`.
        let mut partial = Partial {
            instance: Some(instance.clone()),
            messenger: None,
            surface: None,
        };

        if validation {
            let utils = debug_utils::Instance::new(&entry, &instance);
            // SAFETY: valid instance and create info; user data outlives it.
            let handle = unsafe { utils.create_debug_utils_messenger(&messenger_info, None) }?;
            partial.messenger = Some(Messenger { utils, handle });
        }

        if let Some(w) = window {
            let loader = khr::surface::Instance::new(&entry, &instance);
            // SAFETY: the handles come from a live window that the caller keeps
            // alive for the renderer's lifetime (documented on `new_windowed`).
            let handle = unsafe {
                ash_window::create_surface(&entry, &instance, w.display, w.window, None)
            }?;
            partial.surface = Some(Surface { loader, handle });
        }

        let (physical, queue_family) = pick_physical(&instance, partial.surface.as_ref(), gpu)?;

        // SAFETY: `physical` belongs to `instance`.
        let props = unsafe { instance.get_physical_device_properties(physical) };
        tracing::info!(
            gpu = %props.device_name_as_c_str().map(CStr::to_string_lossy).unwrap_or_default(),
            kind = ?DeviceKind::from_vk(props.device_type),
            validation,
            "Vulkan device selected"
        );

        let mut device_exts = vec![khr::dynamic_rendering::NAME.as_ptr()];
        if partial.surface.is_some() {
            device_exts.push(khr::swapchain::NAME.as_ptr());
        }
        #[cfg(target_os = "macos")]
        if has_device_extension(&instance, physical, khr::portability_subset::NAME) {
            device_exts.push(khr::portability_subset::NAME.as_ptr());
        }
        // Optional: whatever the device offers, never a reason to fail.
        let extensions = optional_extensions(&instance, physical);
        extensions.log();
        device_exts.extend(extensions.to_enable().into_iter().map(CStr::as_ptr));
        let priorities = [1.0f32];
        let queue_info = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(queue_family)
            .queue_priorities(&priorities)];
        let mut dynamic_rendering_feature =
            vk::PhysicalDeviceDynamicRenderingFeaturesKHR::default().dynamic_rendering(true);
        // Water and weather write different subsets of the scene targets in one
        // rendering, which needs a blend state per attachment.
        let features = vk::PhysicalDeviceFeatures::default().independent_blend(true);
        let device_info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queue_info)
            .enabled_features(&features)
            .enabled_extension_names(&device_exts)
            .push_next(&mut dynamic_rendering_feature);
        // SAFETY: extensions and features were checked by `pick_physical`.
        let device = unsafe { instance.create_device(physical, &device_info, None) }?;
        // SAFETY: the queue family and index were requested above.
        let queue = unsafe { device.get_device_queue(queue_family, 0) };
        let dynamic_rendering = khr::dynamic_rendering::Device::new(&instance, &device);

        let allocator = Allocator::new(&AllocatorCreateDesc {
            instance: instance.clone(),
            device: device.clone(),
            physical_device: physical,
            debug_settings: Default::default(),
            buffer_device_address: false,
            allocation_sizes: Default::default(),
        });
        let allocator = match allocator {
            Ok(a) => a,
            Err(e) => {
                // SAFETY: nothing was created from the device yet.
                unsafe { device.destroy_device(None) };
                return Err(e.into());
            }
        };

        let (messenger, surface) = partial.disarm();
        Ok(Self {
            _entry: entry,
            instance,
            messenger,
            messages,
            surface,
            physical,
            device,
            queue,
            queue_family,
            dynamic_rendering,
            extensions,
            allocator: ManuallyDrop::new(allocator),
        })
    }

    pub(crate) fn validation_enabled(&self) -> bool {
        self.messenger.is_some()
    }

    pub(crate) fn validation_messages(&self) -> u32 {
        self.messages.load(Ordering::SeqCst)
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        // SAFETY: the renderer destroyed every object created from the device
        // before dropping the context; the allocator goes before the device.
        unsafe {
            let _ = self.device.device_wait_idle();
            ManuallyDrop::drop(&mut self.allocator);
            self.device.destroy_device(None);
            if let Some(s) = self.surface.take() {
                s.loader.destroy_surface(s.handle, None);
            }
            if let Some(m) = self.messenger.take() {
                m.utils.destroy_debug_utils_messenger(m.handle, None);
            }
            self.instance.destroy_instance(None);
        }
    }
}

/// Instance-level objects owned during construction, released on early error.
struct Partial {
    instance: Option<Instance>,
    messenger: Option<Messenger>,
    surface: Option<Surface>,
}

impl Partial {
    fn disarm(mut self) -> (Option<Messenger>, Option<Surface>) {
        self.instance = None;
        (self.messenger.take(), self.surface.take())
    }
}

impl Drop for Partial {
    fn drop(&mut self) {
        let Some(instance) = self.instance.take() else {
            return;
        };
        // SAFETY: construction failed; these objects have no other users.
        unsafe {
            if let Some(s) = self.surface.take() {
                s.loader.destroy_surface(s.handle, None);
            }
            if let Some(m) = self.messenger.take() {
                m.utils.destroy_debug_utils_messenger(m.handle, None);
            }
            instance.destroy_instance(None);
        }
    }
}

/// Graphics (and present, when windowed) queue family of a device.
fn queue_family(
    instance: &Instance,
    physical: vk::PhysicalDevice,
    surface: Option<&Surface>,
) -> Option<u32> {
    // SAFETY: `physical` belongs to `instance`.
    let families = unsafe { instance.get_physical_device_queue_family_properties(physical) };
    (0..families.len() as u32).find(|&i| {
        families[i as usize]
            .queue_flags
            .contains(vk::QueueFlags::GRAPHICS)
            && surface.is_none_or(|s| {
                // SAFETY: valid device, family index and surface.
                unsafe {
                    s.loader
                        .get_physical_device_surface_support(physical, i, s.handle)
                }
                .unwrap_or(false)
            })
    })
}

fn supports_dynamic_rendering(instance: &Instance, physical: vk::PhysicalDevice) -> bool {
    if !has_device_extension(instance, physical, khr::dynamic_rendering::NAME) {
        return false;
    }
    let mut dynamic = vk::PhysicalDeviceDynamicRenderingFeaturesKHR::default();
    let mut features = vk::PhysicalDeviceFeatures2::default().push_next(&mut dynamic);
    // SAFETY: core 1.1 query on a device of a 1.2 instance.
    unsafe { instance.get_physical_device_features2(physical, &mut features) };
    dynamic.dynamic_rendering == vk::TRUE
}

fn supports_independent_blend(instance: &Instance, physical: vk::PhysicalDevice) -> bool {
    // SAFETY: `physical` was enumerated from `instance`.
    unsafe { instance.get_physical_device_features(physical) }.independent_blend == vk::TRUE
}

fn pick_physical(
    instance: &Instance,
    surface: Option<&Surface>,
    gpu: GpuPreference,
) -> Result<(vk::PhysicalDevice, u32), RenderError> {
    // SAFETY: valid instance.
    let devices = unsafe { instance.enumerate_physical_devices() }?;
    let mut families = Vec::with_capacity(devices.len());
    let candidates: Vec<Candidate> = devices
        .iter()
        .map(|&physical| {
            // SAFETY: `physical` was just enumerated.
            let props = unsafe { instance.get_physical_device_properties(physical) };
            let family = queue_family(instance, physical, surface);
            families.push(family);
            let suitable = props.api_version >= vk::API_VERSION_1_2
                && family.is_some()
                && supports_dynamic_rendering(instance, physical)
                && supports_independent_blend(instance, physical)
                && (surface.is_none()
                    || has_device_extension(instance, physical, khr::swapchain::NAME));
            Candidate {
                kind: DeviceKind::from_vk(props.device_type),
                suitable,
            }
        })
        .collect();
    let index = select::pick_device(&candidates, gpu).ok_or(RenderError::NoSuitableGpu)?;
    let family = families[index].ok_or(RenderError::NoSuitableGpu)?;
    Ok((devices[index], family))
}
