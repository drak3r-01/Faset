//! Optional Vulkan device extensions the renderer enables when the GPU
//! offers them. None is required: a device without them is created all the
//! same, and the features built on them fall back.
//!
//! `faset-upscale` needs the three of them for DLSS (NGX loads CUDA kernels
//! through `VK_NVX_binary_import`, converts image views to handles with
//! `VK_NVX_image_view_handle` and pushes descriptors with
//! `VK_KHR_push_descriptor`); it checks them by asking the device for one
//! command of each, which only resolves for an enabled extension.
//!
//! No Vulkan call here: the choice is a pure function of the extension names
//! a device offers, so it is unit-testable without a GPU.

use std::ffi::CStr;

use ash::{khr, nvx};

/// Which of the optional device extensions the device was created with.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct DeviceExtensions {
    /// `VK_NVX_binary_import`.
    pub nvx_binary_import: bool,
    /// `VK_NVX_image_view_handle`.
    pub nvx_image_view_handle: bool,
    /// `VK_KHR_push_descriptor`.
    pub khr_push_descriptor: bool,
}

impl DeviceExtensions {
    /// Every optional extension, with its Vulkan name, in a fixed order.
    pub const ALL: [(&'static str, &'static CStr); 3] = [
        ("VK_NVX_binary_import", nvx::binary_import::NAME),
        ("VK_NVX_image_view_handle", nvx::image_view_handle::NAME),
        ("VK_KHR_push_descriptor", khr::push_descriptor::NAME),
    ];

    /// The extensions to enable among the `offered` ones: all the optional
    /// extensions that appear in it.
    pub fn from_offered<'a>(offered: impl IntoIterator<Item = &'a CStr>) -> Self {
        let mut found = Self::default();
        for name in offered {
            if name == nvx::binary_import::NAME {
                found.nvx_binary_import = true;
            } else if name == nvx::image_view_handle::NAME {
                found.nvx_image_view_handle = true;
            } else if name == khr::push_descriptor::NAME {
                found.khr_push_descriptor = true;
            }
        }
        found
    }

    fn flags(self) -> [bool; 3] {
        [
            self.nvx_binary_import,
            self.nvx_image_view_handle,
            self.khr_push_descriptor,
        ]
    }

    /// True when the three extensions DLSS needs are enabled.
    pub fn dlss_ready(self) -> bool {
        self.flags().iter().all(|&on| on)
    }

    /// Names of the enabled extensions.
    pub fn enabled(self) -> Vec<&'static str> {
        self.select(true)
    }

    /// Names of the optional extensions the device was created without.
    pub fn missing(self) -> Vec<&'static str> {
        self.select(false)
    }

    fn select(self, on: bool) -> Vec<&'static str> {
        Self::ALL
            .iter()
            .zip(self.flags())
            .filter(|(_, flag)| *flag == on)
            .map(|((name, _), _)| *name)
            .collect()
    }

    /// Extension names to pass to device creation.
    pub(crate) fn to_enable(self) -> Vec<&'static CStr> {
        Self::ALL
            .iter()
            .zip(self.flags())
            .filter(|(_, on)| *on)
            .map(|((_, name), _)| *name)
            .collect()
    }

    /// Logs what was enabled and what the device lacks.
    pub(crate) fn log(self) {
        let enabled = self.enabled();
        let missing = self.missing();
        if !enabled.is_empty() {
            tracing::info!(extensions = ?enabled, "optional Vulkan device extensions enabled");
        }
        if !missing.is_empty() {
            tracing::info!(
                extensions = ?missing,
                dlss_ready = self.dlss_ready(),
                "optional Vulkan device extensions not offered by the device"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offered(names: &[&'static CStr]) -> DeviceExtensions {
        DeviceExtensions::from_offered(names.iter().copied())
    }

    #[test]
    fn a_device_without_them_gets_none() {
        let none = offered(&[khr::swapchain::NAME, khr::dynamic_rendering::NAME]);
        assert_eq!(none, DeviceExtensions::default());
        assert!(!none.dlss_ready());
        assert!(none.enabled().is_empty());
        assert!(none.to_enable().is_empty());
        assert_eq!(none.missing().len(), 3);
    }

    #[test]
    fn an_nvidia_device_gets_all_three() {
        let all = offered(&[
            khr::swapchain::NAME,
            nvx::binary_import::NAME,
            nvx::image_view_handle::NAME,
            khr::push_descriptor::NAME,
        ]);
        assert!(all.dlss_ready());
        assert!(all.missing().is_empty());
        assert_eq!(
            all.enabled(),
            [
                "VK_NVX_binary_import",
                "VK_NVX_image_view_handle",
                "VK_KHR_push_descriptor"
            ]
        );
        assert_eq!(all.to_enable().len(), 3);
    }

    #[test]
    fn a_partial_offer_enables_only_what_exists() {
        let some = offered(&[khr::push_descriptor::NAME]);
        assert!(!some.dlss_ready());
        assert!(some.khr_push_descriptor);
        assert_eq!(some.enabled(), ["VK_KHR_push_descriptor"]);
        assert_eq!(
            some.missing(),
            ["VK_NVX_binary_import", "VK_NVX_image_view_handle"]
        );
        assert_eq!(some.to_enable(), [khr::push_descriptor::NAME]);
    }

    #[test]
    fn names_match_the_vulkan_spelling_the_upscaler_checks() {
        for (name, cname) in DeviceExtensions::ALL {
            assert_eq!(cname.to_str(), Ok(name));
        }
    }
}
