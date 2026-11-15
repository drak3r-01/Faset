//! Headless Vulkan test on Mesa lavapipe: instance, device, clear one
//! offscreen image, read one pixel back. No window, no surface. Any
//! validation-layer message fails the test.

use std::ffi::{c_void, CStr};
use std::sync::atomic::{AtomicUsize, Ordering};

use ash::{ext::debug_utils, vk, Entry};

static VALIDATION_MESSAGES: AtomicUsize = AtomicUsize::new(0);

unsafe extern "system" fn on_message(
    severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    _kind: vk::DebugUtilsMessageTypeFlagsEXT,
    data: *const vk::DebugUtilsMessengerCallbackDataEXT<'_>,
    _user: *mut c_void,
) -> vk::Bool32 {
    if severity.intersects(
        vk::DebugUtilsMessageSeverityFlagsEXT::WARNING
            | vk::DebugUtilsMessageSeverityFlagsEXT::ERROR,
    ) {
        VALIDATION_MESSAGES.fetch_add(1, Ordering::SeqCst);
        let msg = unsafe { CStr::from_ptr((*data).p_message) };
        eprintln!("VALIDATION: {}", msg.to_string_lossy());
    }
    vk::FALSE
}

#[test]
fn clear_and_read_one_pixel_on_lavapipe() {
    const COLOR: [u8; 4] = [255, 128, 0, 255];
    unsafe {
        let entry = Entry::load().expect("Vulkan loader (libvulkan) not found");
        let layer = c"VK_LAYER_KHRONOS_validation";
        let layers = [layer.as_ptr()];
        let exts = [debug_utils::NAME.as_ptr()];
        let app = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_1);
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
            .pfn_user_callback(Some(on_message));
        let instance = entry
            .create_instance(
                &vk::InstanceCreateInfo::default()
                    .application_info(&app)
                    .enabled_layer_names(&layers)
                    .enabled_extension_names(&exts)
                    .push_next(&mut messenger_info),
                None,
            )
            .expect("instance with validation layer");
        let utils = debug_utils::Instance::new(&entry, &instance);
        let messenger = utils
            .create_debug_utils_messenger(&messenger_info, None)
            .unwrap();

        let phys = instance.enumerate_physical_devices().unwrap()[0];
        let props = instance.get_physical_device_properties(phys);
        assert_eq!(
            props.device_type,
            vk::PhysicalDeviceType::CPU,
            "expected lavapipe (CPU device)"
        );
        let family = instance
            .get_physical_device_queue_family_properties(phys)
            .iter()
            .position(|q| q.queue_flags.contains(vk::QueueFlags::GRAPHICS))
            .expect("graphics queue") as u32;
        let prio = [1.0f32];
        let qinfo = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(family)
            .queue_priorities(&prio)];
        let device = instance
            .create_device(
                phys,
                &vk::DeviceCreateInfo::default().queue_create_infos(&qinfo),
                None,
            )
            .unwrap();
        let queue = device.get_device_queue(family, 0);
        let mem_props = instance.get_physical_device_memory_properties(phys);
        let find_mem = |bits: u32, flags: vk::MemoryPropertyFlags| {
            (0..mem_props.memory_type_count)
                .find(|&i| {
                    bits & (1 << i) != 0
                        && mem_props.memory_types[i as usize]
                            .property_flags
                            .contains(flags)
                })
                .expect("memory type")
        };

        let extent = vk::Extent3D {
            width: 4,
            height: 4,
            depth: 1,
        };
        let image = device
            .create_image(
                &vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(vk::Format::R8G8B8A8_UNORM)
                    .extent(extent)
                    .mip_levels(1)
                    .array_layers(1)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .usage(vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::TRANSFER_SRC)
                    .initial_layout(vk::ImageLayout::UNDEFINED),
                None,
            )
            .unwrap();
        let req = device.get_image_memory_requirements(image);
        let image_mem = device
            .allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(req.size)
                    .memory_type_index(find_mem(
                        req.memory_type_bits,
                        vk::MemoryPropertyFlags::DEVICE_LOCAL,
                    )),
                None,
            )
            .unwrap();
        device.bind_image_memory(image, image_mem, 0).unwrap();

        let buffer = device
            .create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(4 * 4 * 4)
                    .usage(vk::BufferUsageFlags::TRANSFER_DST),
                None,
            )
            .unwrap();
        let breq = device.get_buffer_memory_requirements(buffer);
        let buf_mem = device
            .allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(breq.size)
                    .memory_type_index(find_mem(
                        breq.memory_type_bits,
                        vk::MemoryPropertyFlags::HOST_VISIBLE
                            | vk::MemoryPropertyFlags::HOST_COHERENT,
                    )),
                None,
            )
            .unwrap();
        device.bind_buffer_memory(buffer, buf_mem, 0).unwrap();

        let pool = device
            .create_command_pool(
                &vk::CommandPoolCreateInfo::default().queue_family_index(family),
                None,
            )
            .unwrap();
        let cmd = device
            .allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(pool)
                    .command_buffer_count(1),
            )
            .unwrap()[0];
        device
            .begin_command_buffer(cmd, &vk::CommandBufferBeginInfo::default())
            .unwrap();
        let range = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .level_count(1)
            .layer_count(1);
        let barrier = |old, new, src, dst| {
            vk::ImageMemoryBarrier::default()
                .old_layout(old)
                .new_layout(new)
                .src_access_mask(src)
                .dst_access_mask(dst)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(image)
                .subresource_range(range)
        };
        device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::TRANSFER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[barrier(
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                vk::AccessFlags::empty(),
                vk::AccessFlags::TRANSFER_WRITE,
            )],
        );
        let clear = vk::ClearColorValue {
            float32: [1.0, 128.0 / 255.0, 0.0, 1.0],
        };
        device.cmd_clear_color_image(
            cmd,
            image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            &clear,
            &[range],
        );
        device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::TRANSFER,
            vk::PipelineStageFlags::TRANSFER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[barrier(
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                vk::AccessFlags::TRANSFER_WRITE,
                vk::AccessFlags::TRANSFER_READ,
            )],
        );
        let region = vk::BufferImageCopy::default()
            .image_subresource(
                vk::ImageSubresourceLayers::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .layer_count(1),
            )
            .image_extent(extent);
        device.cmd_copy_image_to_buffer(
            cmd,
            image,
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            buffer,
            &[region],
        );
        device.end_command_buffer(cmd).unwrap();
        let cmds = [cmd];
        device
            .queue_submit(
                queue,
                &[vk::SubmitInfo::default().command_buffers(&cmds)],
                vk::Fence::null(),
            )
            .unwrap();
        device.queue_wait_idle(queue).unwrap();

        let ptr = device
            .map_memory(buf_mem, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())
            .unwrap() as *const u8;
        let pixel = [*ptr, *ptr.add(1), *ptr.add(2), *ptr.add(3)];
        device.unmap_memory(buf_mem);

        device.destroy_command_pool(pool, None);
        device.destroy_buffer(buffer, None);
        device.free_memory(buf_mem, None);
        device.destroy_image(image, None);
        device.free_memory(image_mem, None);
        device.destroy_device(None);
        utils.destroy_debug_utils_messenger(messenger, None);
        instance.destroy_instance(None);

        assert_eq!(pixel, COLOR, "pixel read back from the offscreen image");
        assert_eq!(
            VALIDATION_MESSAGES.load(Ordering::SeqCst),
            0,
            "validation layer reported messages"
        );
    }
}
