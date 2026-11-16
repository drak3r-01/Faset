//! Pure selection logic: GPU scoring, surface format, present mode and
//! swapchain sizing. No Vulkan calls here, so everything is unit-testable.

use ash::vk;

use crate::GpuPreference;

/// Kind of a physical device, ordered by preference through [`DeviceKind::score`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeviceKind {
    Discrete,
    Integrated,
    Virtual,
    /// Software rasteriser such as Mesa lavapipe.
    Cpu,
    Other,
}

impl DeviceKind {
    pub(crate) fn from_vk(kind: vk::PhysicalDeviceType) -> Self {
        match kind {
            vk::PhysicalDeviceType::DISCRETE_GPU => Self::Discrete,
            vk::PhysicalDeviceType::INTEGRATED_GPU => Self::Integrated,
            vk::PhysicalDeviceType::VIRTUAL_GPU => Self::Virtual,
            vk::PhysicalDeviceType::CPU => Self::Cpu,
            _ => Self::Other,
        }
    }

    /// Higher is better: discrete > integrated > virtual > software > other.
    pub(crate) fn score(self) -> u32 {
        match self {
            Self::Discrete => 1000,
            Self::Integrated => 100,
            Self::Virtual => 50,
            Self::Cpu => 10,
            Self::Other => 1,
        }
    }
}

/// One enumerated physical device, reduced to what the choice needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub kind: DeviceKind,
    /// Vulkan 1.2, dynamic rendering, a graphics queue (and present support
    /// when windowed).
    pub suitable: bool,
}

/// Picks a device index (enumeration order). An explicit index wins when that
/// device is suitable; otherwise the highest score wins, first one on ties.
pub(crate) fn pick_device(candidates: &[Candidate], pref: GpuPreference) -> Option<usize> {
    if let GpuPreference::Index(index) = pref {
        let index = index as usize;
        if candidates.get(index).is_some_and(|c| c.suitable) {
            return Some(index);
        }
        tracing::warn!(
            index,
            "preferred GPU is missing or unsuitable, using the automatic choice"
        );
    }
    candidates
        .iter()
        .enumerate()
        .filter(|(_, c)| c.suitable)
        .max_by_key(|&(i, c)| (c.kind.score(), std::cmp::Reverse(i)))
        .map(|(i, _)| i)
}

/// Vsync on: FIFO. Vsync off: MAILBOX, else IMMEDIATE, else FIFO (always
/// available per the Vulkan spec).
pub(crate) fn choose_present_mode(
    vsync: bool,
    available: &[vk::PresentModeKHR],
) -> vk::PresentModeKHR {
    if vsync {
        return vk::PresentModeKHR::FIFO;
    }
    [vk::PresentModeKHR::MAILBOX, vk::PresentModeKHR::IMMEDIATE]
        .into_iter()
        .find(|m| available.contains(m))
        .unwrap_or(vk::PresentModeKHR::FIFO)
}

/// First available 8-bit sRGB format, BGRA preferred over RGBA.
pub(crate) fn choose_surface_format(
    available: &[vk::SurfaceFormatKHR],
) -> Option<vk::SurfaceFormatKHR> {
    const PREFERRED: [vk::Format; 2] = [vk::Format::B8G8R8A8_SRGB, vk::Format::R8G8B8A8_SRGB];
    PREFERRED.iter().find_map(|&format| {
        available
            .iter()
            .copied()
            .find(|s| s.format == format && s.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR)
    })
}

/// Swapchain extent: the surface's when it imposes one, else the window size
/// clamped to the allowed range.
pub(crate) fn swapchain_extent(
    caps: &vk::SurfaceCapabilitiesKHR,
    (width, height): (u32, u32),
) -> vk::Extent2D {
    if caps.current_extent.width != u32::MAX {
        return caps.current_extent;
    }
    vk::Extent2D {
        width: width.clamp(caps.min_image_extent.width, caps.max_image_extent.width),
        height: height.clamp(caps.min_image_extent.height, caps.max_image_extent.height),
    }
}

/// One image more than the minimum, within the maximum (0 = unbounded).
pub(crate) fn swapchain_image_count(caps: &vk::SurfaceCapabilitiesKHR) -> u32 {
    let wanted = caps.min_image_count + 1;
    if caps.max_image_count == 0 {
        wanted
    } else {
        wanted.min(caps.max_image_count)
    }
}

/// True when nothing can be drawn (minimised window or 0x0 target).
pub(crate) fn is_zero_area((width, height): (u32, u32)) -> bool {
    width == 0 || height == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(kind: DeviceKind) -> Candidate {
        Candidate {
            kind,
            suitable: true,
        }
    }

    #[test]
    fn score_orders_discrete_integrated_cpu() {
        assert!(DeviceKind::Discrete.score() > DeviceKind::Integrated.score());
        assert!(DeviceKind::Integrated.score() > DeviceKind::Cpu.score());
        let cpu_integrated_discrete = [
            dev(DeviceKind::Cpu),
            dev(DeviceKind::Integrated),
            dev(DeviceKind::Discrete),
        ];
        assert_eq!(
            pick_device(&cpu_integrated_discrete, GpuPreference::Auto),
            Some(2)
        );
        let cpu_integrated = [dev(DeviceKind::Cpu), dev(DeviceKind::Integrated)];
        assert_eq!(pick_device(&cpu_integrated, GpuPreference::Auto), Some(1));
        assert_eq!(
            pick_device(&[dev(DeviceKind::Cpu)], GpuPreference::Auto),
            Some(0)
        );
    }

    #[test]
    fn unsuitable_devices_are_skipped_and_ties_keep_first() {
        let mut discrete = dev(DeviceKind::Discrete);
        discrete.suitable = false;
        let list = [
            discrete,
            dev(DeviceKind::Integrated),
            dev(DeviceKind::Integrated),
        ];
        assert_eq!(pick_device(&list, GpuPreference::Auto), Some(1));
        assert_eq!(pick_device(&[discrete], GpuPreference::Auto), None);
        assert_eq!(pick_device(&[], GpuPreference::Auto), None);
    }

    #[test]
    fn explicit_index_overrides_score_when_suitable() {
        let mut list = [dev(DeviceKind::Discrete), dev(DeviceKind::Cpu)];
        assert_eq!(pick_device(&list, GpuPreference::Index(1)), Some(1));
        list[1].suitable = false;
        assert_eq!(pick_device(&list, GpuPreference::Index(1)), Some(0));
        assert_eq!(pick_device(&list, GpuPreference::Index(7)), Some(0));
    }

    #[test]
    fn present_mode_follows_vsync() {
        use vk::PresentModeKHR as P;
        let all = [P::FIFO, P::IMMEDIATE, P::MAILBOX];
        assert_eq!(choose_present_mode(true, &all), P::FIFO);
        assert_eq!(choose_present_mode(false, &all), P::MAILBOX);
        assert_eq!(
            choose_present_mode(false, &[P::FIFO, P::IMMEDIATE]),
            P::IMMEDIATE
        );
        assert_eq!(choose_present_mode(false, &[P::FIFO]), P::FIFO);
        assert_eq!(choose_present_mode(false, &[]), P::FIFO);
        assert_eq!(choose_present_mode(true, &[P::MAILBOX]), P::FIFO);
    }

    #[test]
    fn surface_format_is_srgb() {
        let fmt = |format| vk::SurfaceFormatKHR {
            format,
            color_space: vk::ColorSpaceKHR::SRGB_NONLINEAR,
        };
        let both = [
            fmt(vk::Format::B8G8R8A8_UNORM),
            fmt(vk::Format::R8G8B8A8_SRGB),
            fmt(vk::Format::B8G8R8A8_SRGB),
        ];
        assert_eq!(
            choose_surface_format(&both).map(|f| f.format),
            Some(vk::Format::B8G8R8A8_SRGB)
        );
        assert_eq!(
            choose_surface_format(&both[..2]).map(|f| f.format),
            Some(vk::Format::R8G8B8A8_SRGB)
        );
        assert_eq!(choose_surface_format(&both[..1]), None);
    }

    #[test]
    fn extent_and_image_count() {
        let mut caps = vk::SurfaceCapabilitiesKHR {
            current_extent: vk::Extent2D {
                width: u32::MAX,
                height: u32::MAX,
            },
            min_image_extent: vk::Extent2D {
                width: 1,
                height: 1,
            },
            max_image_extent: vk::Extent2D {
                width: 4096,
                height: 4096,
            },
            min_image_count: 2,
            max_image_count: 0,
            ..Default::default()
        };
        assert_eq!(
            swapchain_extent(&caps, (8000, 600)),
            vk::Extent2D {
                width: 4096,
                height: 600
            }
        );
        assert_eq!(swapchain_image_count(&caps), 3);
        caps.max_image_count = 2;
        assert_eq!(swapchain_image_count(&caps), 2);
        caps.current_extent = vk::Extent2D {
            width: 0,
            height: 0,
        };
        assert!(is_zero_area({
            let e = swapchain_extent(&caps, (800, 600));
            (e.width, e.height)
        }));
        assert!(is_zero_area((0, 600)) && is_zero_area((800, 0)));
        assert!(!is_zero_area((1, 1)));
    }
}
