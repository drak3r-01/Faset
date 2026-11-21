//! Post-processing chain on the HDR colour buffer: contours, bloom,
//! tonemapping, grading, screen effects and FXAA.
//!
//! Every pass draws one full-screen triangle (`post.wgsl`) into an RGBA16F
//! image; the chain ping-pongs between two images and hands the last one to
//! the final blit, which encodes it to sRGB. Passes whose [`PostSettings`]
//! flag is off, or whose intensities are all 0, are not recorded at all.

use ash::vk;
use serde::{Deserialize, Serialize};

use crate::ambience::AmbienceProfile;
use crate::context::Context;
use crate::frame::OwnedImage;
use crate::gpu::{create_image, image_barriers, ImageBarrier};
use crate::opaque::HDR_FORMAT;
use crate::RenderError;

/// Downsamples of the bloom chain (half resolution down to 1/64).
pub const BLOOM_LEVELS: usize = 6;
/// Bytes of the push constant block of every post pass (eight `vec4`).
const PUSH_SIZE: u32 = 128;

/// Which effects of the chain run. TOML loading is lot 6.4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PostSettings {
    pub contours: bool,
    pub bloom: bool,
    pub tonemap: bool,
    pub grading: bool,
    pub screen_effects: bool,
    pub fxaa: bool,
}

impl Default for PostSettings {
    fn default() -> Self {
        Self {
            contours: true,
            bloom: true,
            tonemap: true,
            grading: true,
            screen_effects: true,
            fxaa: true,
        }
    }
}

impl PostSettings {
    /// Every effect off: the chain only copies the HDR buffer to the target.
    pub const NONE: Self = Self {
        contours: false,
        bloom: false,
        tonemap: false,
        grading: false,
        screen_effects: false,
        fxaa: false,
    };
}

/// Per-frame values of the post effects. [`PostParams::from_profile`] derives
/// them from an ambience profile; an intensity of 0 disables its effect.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PostParams {
    /// Exposure multiplier applied before the tonemapping curve, > 0.
    pub exposure: f32,
    /// Luminance above which pixels bloom.
    pub bloom_threshold: f32,
    /// Width of the soft knee around the threshold, >= 0.
    pub bloom_knee: f32,
    /// Bloom added back to the image, 0 disables it.
    pub bloom_intensity: f32,
    pub lift: [f32; 3],
    pub gamma: [f32; 3],
    pub gain: [f32; 3],
    pub saturation: f32,
    /// Hue rotation in radians.
    pub hue_shift: f32,
    /// Sobel step in pixels, >= 1.
    pub contour_thickness: f32,
    /// Contour opacity in [0, 1], 0 disables contours.
    pub contour_strength: f32,
    pub contour_color: [f32; 3],
    /// Relative depth gradient from which a depth edge shows.
    pub contour_depth_threshold: f32,
    /// Normal gradient from which a crease shows.
    pub contour_normal_threshold: f32,
    /// Corner darkening in [0, 1].
    pub vignette: f32,
    /// Radial red/blue split in UV units.
    pub chromatic_aberration: f32,
    /// Film grain amplitude.
    pub grain: f32,
    /// Underwater image wobble amplitude in UV units (0 above water).
    pub ripple: f32,
    /// Damage flash in [0, 1].
    pub damage_flash: f32,
    pub damage_color: [f32; 3],
    /// Speed line strength in [0, 1].
    pub speed_lines: f32,
}

impl Default for PostParams {
    /// Neutral values: exposure 1, identity grading, no effect.
    fn default() -> Self {
        Self {
            exposure: 1.0,
            bloom_threshold: 1.0,
            bloom_knee: 0.5,
            bloom_intensity: 0.0,
            lift: [0.0; 3],
            gamma: [1.0; 3],
            gain: [1.0; 3],
            saturation: 1.0,
            hue_shift: 0.0,
            contour_thickness: 1.0,
            contour_strength: 0.0,
            contour_color: [0.02, 0.02, 0.04],
            contour_depth_threshold: 0.08,
            contour_normal_threshold: 0.3,
            vignette: 0.0,
            chromatic_aberration: 0.0,
            grain: 0.0,
            ripple: 0.0,
            damage_flash: 0.0,
            damage_color: [0.8, 0.05, 0.05],
            speed_lines: 0.0,
        }
    }
}

impl PostParams {
    /// Exposure, bloom and grading come from the profile; the other effects
    /// get a mild look (a little vignette, grain and chromatic aberration,
    /// light contours), the underwater ripple when `underwater`. Damage flash
    /// and speed lines are 0: they are driven by gameplay
    /// ([`crate::ScreenEffects`]).
    pub fn from_profile(profile: &AmbienceProfile, underwater: bool) -> Self {
        let g = &profile.grading;
        Self {
            exposure: profile.exposure,
            bloom_threshold: g.bloom_threshold,
            bloom_knee: 0.5 * g.bloom_threshold.max(0.1),
            bloom_intensity: g.bloom_intensity,
            lift: g.lift,
            gamma: g.gamma,
            gain: g.gain,
            saturation: g.saturation,
            hue_shift: g.hue_shift,
            contour_strength: 0.35,
            vignette: 0.25,
            chromatic_aberration: 0.0015,
            grain: 0.02,
            ripple: if underwater { 0.003 } else { 0.0 },
            ..Self::default()
        }
    }

    fn effects_active(&self) -> bool {
        self.ripple > 0.0
            || self.chromatic_aberration > 0.0
            || self.vignette > 0.0
            || self.grain > 0.0
            || self.speed_lines > 0.0
            || self.damage_flash > 0.0
    }
}

/// Gameplay-driven screen effects, set on the [`crate::RenderWorld`].
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ScreenEffects {
    /// Red flash on damage, in [0, 1].
    pub damage_flash: f32,
    /// Radial speed streaks, in [0, 1].
    pub speed_lines: f32,
}

/// The approximate ACES filmic curve of the plan (one channel).
///
/// `T(0) = 0`, it is increasing, and it stays below 1 up to x = 7; its
/// asymptote is 1.033, so the shader clamps the result to [0, 1].
pub fn aces_tonemap(x: f32) -> f32 {
    (x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14)
}

/// Sizes of the bloom levels for a target of `extent`: level 0 is half the
/// size, each next one half again (at least 1 pixel).
pub fn bloom_extents(extent: (u32, u32)) -> [(u32, u32); BLOOM_LEVELS] {
    let mut out = [(1, 1); BLOOM_LEVELS];
    let (mut w, mut h) = extent;
    for e in &mut out {
        w = (w / 2).max(1);
        h = (h / 2).max(1);
        *e = (w, h);
    }
    out
}

type Push = [[f32; 4]; 8];

/// Which image holds the chain's current result.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Cur {
    Hdr,
    A,
    B,
}

/// Pipelines, layouts and the sampler; created at the first use.
pub(crate) struct PostPipelines {
    set_layout: vk::DescriptorSetLayout,
    layout: vk::PipelineLayout,
    sampler: vk::Sampler,
    contours: vk::Pipeline,
    down: vk::Pipeline,
    up: vk::Pipeline,
    tonemap: vk::Pipeline,
    effects: vk::Pipeline,
    fxaa: vk::Pipeline,
}

/// Images and descriptor sets of one target size.
pub(crate) struct PostTargets {
    extent: vk::Extent2D,
    ping: [OwnedImage; 2],
    bloom: Vec<OwnedImage>,
    pool: vk::DescriptorPool,
    /// `[hdr, depth, normal]`.
    contour_set: vk::DescriptorSet,
    /// The same image three times, for `Cur::Hdr`, `A`, `B`.
    any_sets: [vk::DescriptorSet; 3],
    /// `[colour, bloom level 0, colour]` for `Cur::Hdr`, `A`.
    tonemap_sets: [vk::DescriptorSet; 2],
    /// One bloom level three times.
    bloom_sets: Vec<vk::DescriptorSet>,
}

/// The chain: settings, pipelines and the targets.
pub(crate) struct PostChain {
    pub settings: PostSettings,
    pipelines: PostPipelines,
    pub targets: Option<PostTargets>,
}

/// Images of the opaque pass the chain reads.
pub(crate) struct PostInputs {
    pub hdr: vk::Image,
    pub hdr_view: vk::ImageView,
    pub depth_view: vk::ImageView,
    pub normal_view: vk::ImageView,
}

impl PostChain {
    pub(crate) fn new(ctx: &Context, settings: PostSettings) -> Result<Self, RenderError> {
        Ok(Self {
            settings,
            pipelines: PostPipelines::new(&ctx.device)?,
            targets: None,
        })
    }

    /// True when at least one pass would be recorded for these parameters.
    pub(crate) fn is_active(&self, params: &PostParams) -> bool {
        self.passes(params) != Passes::default()
    }

    fn passes(&self, p: &PostParams) -> Passes {
        let s = &self.settings;
        let bloom = s.bloom && p.bloom_intensity > 0.0;
        Passes {
            contours: s.contours && p.contour_strength > 0.0,
            bloom,
            tonemap: s.tonemap || s.grading || bloom,
            effects: s.screen_effects && p.effects_active(),
            fxaa: s.fxaa,
        }
    }

    /// (Re)creates the targets when the size changed; waits for the device.
    pub(crate) fn ensure_targets(
        &mut self,
        ctx: &mut Context,
        extent: vk::Extent2D,
        inputs: &PostInputs,
    ) -> Result<(), RenderError> {
        if let Some(t) = &self.targets {
            if t.extent == extent {
                return Ok(());
            }
        }
        // SAFETY: valid device; makes the old targets unused. Only happens
        // on a resize.
        unsafe { ctx.device.device_wait_idle() }?;
        if let Some(old) = self.targets.take() {
            old.destroy(ctx);
        }
        self.targets = Some(PostTargets::new(ctx, &self.pipelines, extent, inputs)?);
        Ok(())
    }

    pub(crate) fn destroy(&mut self, ctx: &mut Context) {
        if let Some(t) = self.targets.take() {
            t.destroy(ctx);
        }
        self.pipelines.destroy(&ctx.device);
    }

    /// Records the chain. `inputs.hdr` must be in `SHADER_READ_ONLY_OPTIMAL`,
    /// the depth in `DEPTH_STENCIL_READ_ONLY_OPTIMAL` and the normals in
    /// `SHADER_READ_ONLY_OPTIMAL`. Returns the image holding the result, in
    /// `SHADER_READ_ONLY_OPTIMAL` (the input itself when nothing ran).
    ///
    /// # Safety
    /// `cmd` must be recording on a queue of `ctx.device`, outside a
    /// rendering, and the targets must exist ([`Self::ensure_targets`]).
    pub(crate) unsafe fn record(
        &self,
        ctx: &Context,
        cmd: vk::CommandBuffer,
        inputs: &PostInputs,
        params: &PostParams,
        near: f32,
        time: f32,
    ) -> vk::Image {
        let t = self.targets.as_ref().expect("targets created");
        let p = self.passes(params);
        let pl = &self.pipelines;
        let mut rec = Recorder {
            ctx,
            cmd,
            layout: pl.layout,
        };
        let image_of = |c: Cur| match c {
            Cur::Hdr => inputs.hdr,
            Cur::A => t.ping[0].image,
            Cur::B => t.ping[1].image,
        };
        let free = |c: Cur| if c == Cur::A { Cur::B } else { Cur::A };
        let idx = |c: Cur| match c {
            Cur::Hdr => 0,
            Cur::A => 1,
            Cur::B => 2,
        };
        let extent_vec = (t.extent.width as f32, t.extent.height as f32);
        let ext = (t.extent.width, t.extent.height);
        let mut cur = Cur::Hdr;

        // Contours: Sobel on linear depth and normals.
        if p.contours {
            let target = &t.ping[0];
            let mut push = [[0.0; 4]; 8];
            push[0] = [
                params.contour_thickness.max(1.0),
                params.contour_strength.clamp(0.0, 1.0),
                params.contour_depth_threshold,
                params.contour_normal_threshold,
            ];
            let c = params.contour_color;
            push[1] = [c[0], c[1], c[2], near];
            // SAFETY: forwarded.
            unsafe {
                rec.to_attachment(target.image, vk::ImageLayout::UNDEFINED);
                rec.draw(pl.contours, t.contour_set, target, ext, &push, false);
                rec.to_read(target.image);
            }
            cur = Cur::A;
        }

        // Bloom: soft threshold, six downsamples, additive upsamples.
        let bloom_exts = bloom_extents((t.extent.width, t.extent.height));
        if p.bloom {
            let src_texel = [1.0 / extent_vec.0, 1.0 / extent_vec.1];
            let mut push = [[0.0; 4]; 8];
            push[0] = [
                src_texel[0],
                src_texel[1],
                params.bloom_threshold.max(0.0),
                params.bloom_knee.max(0.0),
            ];
            // SAFETY: forwarded.
            unsafe {
                let d0 = &t.bloom[0];
                rec.to_attachment(d0.image, vk::ImageLayout::UNDEFINED);
                rec.draw(
                    pl.down,
                    t.any_sets[idx(cur)],
                    d0,
                    bloom_exts[0],
                    &push,
                    false,
                );
                rec.to_read(d0.image);
                for i in 1..BLOOM_LEVELS {
                    let prev = bloom_exts[i - 1];
                    let mut push = [[0.0; 4]; 8];
                    push[0] = [1.0 / prev.0 as f32, 1.0 / prev.1 as f32, 0.0, 0.0];
                    let target = &t.bloom[i];
                    rec.to_attachment(target.image, vk::ImageLayout::UNDEFINED);
                    rec.draw(
                        pl.down,
                        t.bloom_sets[i - 1],
                        target,
                        bloom_exts[i],
                        &push,
                        false,
                    );
                    rec.to_read(target.image);
                }
                for i in (1..BLOOM_LEVELS).rev() {
                    let small = bloom_exts[i];
                    let mut push = [[0.0; 4]; 8];
                    push[0] = [1.0 / small.0 as f32, 1.0 / small.1 as f32, 0.0, 0.0];
                    let target = &t.bloom[i - 1];
                    rec.to_attachment(target.image, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
                    rec.draw(
                        pl.up,
                        t.bloom_sets[i],
                        target,
                        bloom_exts[i - 1],
                        &push,
                        true,
                    );
                    rec.to_read(target.image);
                }
            }
        } else {
            // The tonemap set binds level 0 whatever the settings.
            // SAFETY: forwarded.
            unsafe { rec.to_read_from_undefined(t.bloom[0].image) };
        }

        // Bloom add, exposure, ACES, clamp, grading.
        if p.tonemap {
            let target_cur = free(cur);
            let target = &t.ping[if target_cur == Cur::A { 0 } else { 1 }];
            let mut push = [[0.0; 4]; 8];
            let s = &self.settings;
            let bloom = if p.bloom { params.bloom_intensity } else { 0.0 };
            push[0] = [
                params.exposure.max(1.0e-4),
                bloom,
                f32::from(u8::from(s.tonemap)),
                f32::from(u8::from(s.grading)),
            ];
            push[1] = [
                params.lift[0],
                params.lift[1],
                params.lift[2],
                params.saturation,
            ];
            push[2] = [
                params.gamma[0],
                params.gamma[1],
                params.gamma[2],
                params.hue_shift,
            ];
            push[3] = [params.gain[0], params.gain[1], params.gain[2], 0.0];
            // SAFETY: forwarded.
            unsafe {
                rec.to_attachment(target.image, vk::ImageLayout::UNDEFINED);
                rec.draw(
                    pl.tonemap,
                    t.tonemap_sets[if cur == Cur::Hdr { 0 } else { 1 }],
                    target,
                    ext,
                    &push,
                    false,
                );
                rec.to_read(target.image);
            }
            cur = target_cur;
        }

        // Screen effects.
        if p.effects {
            let target_cur = free(cur);
            let target = &t.ping[if target_cur == Cur::A { 0 } else { 1 }];
            let mut push = [[0.0; 4]; 8];
            push[0] = [
                params.ripple.max(0.0),
                params.chromatic_aberration.max(0.0),
                params.vignette.clamp(0.0, 1.0),
                params.grain.max(0.0),
            ];
            push[1] = [
                params.speed_lines.clamp(0.0, 1.0),
                params.damage_flash.clamp(0.0, 1.0),
                time,
                extent_vec.0 / extent_vec.1,
            ];
            let c = params.damage_color;
            push[2] = [c[0], c[1], c[2], 0.0];
            // SAFETY: forwarded.
            unsafe {
                rec.to_attachment(target.image, vk::ImageLayout::UNDEFINED);
                rec.draw(pl.effects, t.any_sets[idx(cur)], target, ext, &push, false);
                rec.to_read(target.image);
            }
            cur = target_cur;
        }

        // FXAA on the finished image.
        if p.fxaa {
            let target_cur = free(cur);
            let target = &t.ping[if target_cur == Cur::A { 0 } else { 1 }];
            let push = [[0.0; 4]; 8];
            // SAFETY: forwarded.
            unsafe {
                rec.to_attachment(target.image, vk::ImageLayout::UNDEFINED);
                rec.draw(pl.fxaa, t.any_sets[idx(cur)], target, ext, &push, false);
                rec.to_read(target.image);
            }
            cur = target_cur;
        }
        image_of(cur)
    }
}

/// The passes one frame runs.
#[derive(Default, PartialEq, Eq, Clone, Copy)]
struct Passes {
    contours: bool,
    bloom: bool,
    tonemap: bool,
    effects: bool,
    fxaa: bool,
}

/// Command recording helpers of the chain.
struct Recorder<'a> {
    ctx: &'a Context,
    cmd: vk::CommandBuffer,
    layout: vk::PipelineLayout,
}

impl Recorder<'_> {
    unsafe fn barrier(&self, image: vk::Image, old: vk::ImageLayout, new: vk::ImageLayout) {
        // SAFETY: forwarded; conservative stages and accesses.
        unsafe {
            image_barriers(
                &self.ctx.device,
                self.cmd,
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::PipelineStageFlags::ALL_COMMANDS,
                &[ImageBarrier {
                    image,
                    aspect: vk::ImageAspectFlags::COLOR,
                    old,
                    new,
                    src_access: vk::AccessFlags::MEMORY_WRITE,
                    dst_access: vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE,
                }],
            );
        }
    }

    unsafe fn to_attachment(&self, image: vk::Image, old: vk::ImageLayout) {
        // SAFETY: forwarded.
        unsafe { self.barrier(image, old, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL) }
    }

    unsafe fn to_read(&self, image: vk::Image) {
        // SAFETY: forwarded.
        unsafe {
            self.barrier(
                image,
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            );
        }
    }

    unsafe fn to_read_from_undefined(&self, image: vk::Image) {
        // SAFETY: forwarded.
        unsafe {
            self.barrier(
                image,
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            );
        }
    }

    /// One full-screen pass into `target` (in `COLOR_ATTACHMENT_OPTIMAL`);
    /// `keep` loads its contents instead of discarding them.
    unsafe fn draw(
        &mut self,
        pipeline: vk::Pipeline,
        set: vk::DescriptorSet,
        target: &OwnedImage,
        size: (u32, u32),
        push: &Push,
        keep: bool,
    ) {
        let device = &self.ctx.device;
        let area = vk::Rect2D {
            offset: vk::Offset2D::default(),
            extent: vk::Extent2D {
                width: size.0,
                height: size.1,
            },
        };
        let attachments = [vk::RenderingAttachmentInfo::default()
            .image_view(target.view)
            .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .load_op(if keep {
                vk::AttachmentLoadOp::LOAD
            } else {
                vk::AttachmentLoadOp::DONT_CARE
            })
            .store_op(vk::AttachmentStoreOp::STORE)];
        let info = vk::RenderingInfo::default()
            .render_area(area)
            .layer_count(1)
            .color_attachments(&attachments);
        let viewport = vk::Viewport {
            x: 0.0,
            y: 0.0,
            width: size.0 as f32,
            height: size.1 as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        };
        // SAFETY: the caller guarantees a recording command buffer and live
        // objects.
        unsafe {
            self.ctx
                .dynamic_rendering
                .cmd_begin_rendering(self.cmd, &info);
            device.cmd_bind_pipeline(self.cmd, vk::PipelineBindPoint::GRAPHICS, pipeline);
            device.cmd_set_viewport(self.cmd, 0, &[viewport]);
            device.cmd_set_scissor(self.cmd, 0, &[area]);
            device.cmd_bind_descriptor_sets(
                self.cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.layout,
                0,
                &[set],
                &[],
            );
            device.cmd_push_constants(
                self.cmd,
                self.layout,
                vk::ShaderStageFlags::FRAGMENT,
                0,
                bytemuck::cast_slice(push),
            );
            device.cmd_draw(self.cmd, 3, 1, 0, 0);
            self.ctx.dynamic_rendering.cmd_end_rendering(self.cmd);
        }
    }
}

impl PostPipelines {
    fn new(device: &ash::Device) -> Result<Self, RenderError> {
        let mut out = Self {
            set_layout: vk::DescriptorSetLayout::null(),
            layout: vk::PipelineLayout::null(),
            sampler: vk::Sampler::null(),
            contours: vk::Pipeline::null(),
            down: vk::Pipeline::null(),
            up: vk::Pipeline::null(),
            tonemap: vk::Pipeline::null(),
            effects: vk::Pipeline::null(),
            fxaa: vk::Pipeline::null(),
        };
        match out.build(device) {
            Ok(()) => Ok(out),
            Err(e) => {
                out.destroy(device);
                Err(e)
            }
        }
    }

    fn build(&mut self, device: &ash::Device) -> Result<(), RenderError> {
        // SAFETY: valid device and create infos; handles are stored at once
        // so `destroy` releases them on a later failure.
        unsafe {
            self.sampler = device.create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(vk::Filter::LINEAR)
                    .min_filter(vk::Filter::LINEAR)
                    .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
                    .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
                None,
            )?;
            let samplers = [self.sampler];
            let image_binding = |binding| {
                vk::DescriptorSetLayoutBinding::default()
                    .binding(binding)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT)
            };
            let bindings = [
                image_binding(0),
                vk::DescriptorSetLayoutBinding::default()
                    .binding(1)
                    .descriptor_type(vk::DescriptorType::SAMPLER)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT)
                    .immutable_samplers(&samplers),
                image_binding(2),
                image_binding(3),
            ];
            self.set_layout = device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                None,
            )?;
            let set_layouts = [self.set_layout];
            let ranges = [vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::FRAGMENT)
                .offset(0)
                .size(PUSH_SIZE)];
            self.layout = device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(&set_layouts)
                    .push_constant_ranges(&ranges),
                None,
            )?;
        }
        let module = |words: &[u32]| {
            // SAFETY: SPIR-V produced and validated by naga.
            unsafe {
                device
                    .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(words), None)
            }
        };
        let vs = module(faset_shaders::POST_VS)?;
        let result = (|| {
            self.contours = self.pipeline(
                device,
                vs,
                faset_shaders::POST_FS_CONTOURS,
                c"fs_contours",
                false,
            )?;
            self.down =
                self.pipeline(device, vs, faset_shaders::POST_FS_DOWN, c"fs_down", false)?;
            self.up = self.pipeline(device, vs, faset_shaders::POST_FS_UP, c"fs_up", true)?;
            self.tonemap = self.pipeline(
                device,
                vs,
                faset_shaders::POST_FS_TONEMAP,
                c"fs_tonemap",
                false,
            )?;
            self.effects = self.pipeline(
                device,
                vs,
                faset_shaders::POST_FS_EFFECTS,
                c"fs_effects",
                false,
            )?;
            self.fxaa =
                self.pipeline(device, vs, faset_shaders::POST_FS_FXAA, c"fs_fxaa", false)?;
            Ok(())
        })();
        // SAFETY: modules may be destroyed once pipelines are created.
        unsafe { device.destroy_shader_module(vs, None) };
        result
    }

    fn pipeline(
        &self,
        device: &ash::Device,
        vs: vk::ShaderModule,
        fs_words: &[u32],
        entry: &std::ffi::CStr,
        additive: bool,
    ) -> Result<vk::Pipeline, RenderError> {
        // SAFETY: SPIR-V produced and validated by naga.
        let fs = unsafe {
            device.create_shader_module(&vk::ShaderModuleCreateInfo::default().code(fs_words), None)
        }?;
        let stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(vs)
                .name(c"vs_main"),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(fs)
                .name(entry),
        ];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        let raster = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let mut colour = vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA);
        if additive {
            colour = colour
                .blend_enable(true)
                .src_color_blend_factor(vk::BlendFactor::ONE)
                .dst_color_blend_factor(vk::BlendFactor::ONE)
                .color_blend_op(vk::BlendOp::ADD)
                .src_alpha_blend_factor(vk::BlendFactor::ZERO)
                .dst_alpha_blend_factor(vk::BlendFactor::ONE)
                .alpha_blend_op(vk::BlendOp::ADD);
        }
        let blend_attachments = [colour];
        let blend =
            vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let formats = [HDR_FORMAT];
        let mut rendering =
            vk::PipelineRenderingCreateInfo::default().color_attachment_formats(&formats);
        let info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport)
            .rasterization_state(&raster)
            .multisample_state(&multisample)
            .color_blend_state(&blend)
            .dynamic_state(&dynamic)
            .layout(self.layout)
            .push_next(&mut rendering);
        // SAFETY: every pointer in `info` outlives the call.
        let created =
            unsafe { device.create_graphics_pipelines(vk::PipelineCache::null(), &[info], None) };
        // SAFETY: the module is no longer needed once the pipeline exists.
        unsafe { device.destroy_shader_module(fs, None) };
        Ok(created.map_err(|(_, e)| e)?[0])
    }

    fn destroy(&mut self, device: &ash::Device) {
        // SAFETY: the device is idle; null handles are a no-op.
        unsafe {
            for p in [
                self.contours,
                self.down,
                self.up,
                self.tonemap,
                self.effects,
                self.fxaa,
            ] {
                device.destroy_pipeline(p, None);
            }
            device.destroy_pipeline_layout(self.layout, None);
            device.destroy_descriptor_set_layout(self.set_layout, None);
            device.destroy_sampler(self.sampler, None);
        }
        *self = Self {
            set_layout: vk::DescriptorSetLayout::null(),
            layout: vk::PipelineLayout::null(),
            sampler: vk::Sampler::null(),
            contours: vk::Pipeline::null(),
            down: vk::Pipeline::null(),
            up: vk::Pipeline::null(),
            tonemap: vk::Pipeline::null(),
            effects: vk::Pipeline::null(),
            fxaa: vk::Pipeline::null(),
        };
    }
}

impl PostTargets {
    fn new(
        ctx: &mut Context,
        pl: &PostPipelines,
        extent: vk::Extent2D,
        inputs: &PostInputs,
    ) -> Result<Self, RenderError> {
        let usage = vk::ImageUsageFlags::COLOR_ATTACHMENT
            | vk::ImageUsageFlags::SAMPLED
            | vk::ImageUsageFlags::TRANSFER_SRC;
        let color = vk::ImageAspectFlags::COLOR;
        let mut images: Vec<OwnedImage> = Vec::with_capacity(2 + BLOOM_LEVELS);
        let mut specs = vec![("post-a", extent), ("post-b", extent)];
        for (w, h) in bloom_extents((extent.width, extent.height)) {
            specs.push((
                "post-bloom",
                vk::Extent2D {
                    width: w,
                    height: h,
                },
            ));
        }
        for (name, e) in specs {
            match create_image(ctx, name, e, HDR_FORMAT, usage, color) {
                Ok(img) => images.push(img),
                Err(err) => {
                    for mut img in images {
                        img.destroy(ctx);
                    }
                    return Err(err);
                }
            }
        }
        let bloom = images.split_off(2);
        let mut ping = images.into_iter();
        let ping = [
            ping.next().expect("two ping images"),
            ping.next().expect("two ping images"),
        ];
        let mut out = Self {
            extent,
            ping,
            bloom,
            pool: vk::DescriptorPool::null(),
            contour_set: vk::DescriptorSet::null(),
            any_sets: [vk::DescriptorSet::null(); 3],
            tonemap_sets: [vk::DescriptorSet::null(); 2],
            bloom_sets: Vec::new(),
        };
        if let Err(e) = out.make_sets(&ctx.device, pl, inputs) {
            out.destroy(ctx);
            return Err(e);
        }
        Ok(out)
    }

    fn make_sets(
        &mut self,
        device: &ash::Device,
        pl: &PostPipelines,
        inputs: &PostInputs,
    ) -> Result<(), RenderError> {
        let sets_count = 1 + 3 + 2 + BLOOM_LEVELS as u32;
        let sizes = [
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(3 * sets_count),
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::SAMPLER)
                .descriptor_count(sets_count),
        ];
        // SAFETY: valid device and create info.
        self.pool = unsafe {
            device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(sets_count)
                    .pool_sizes(&sizes),
                None,
            )
        }?;
        let layouts = vec![pl.set_layout; sets_count as usize];
        // SAFETY: the pool is sized for these sets.
        let sets = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(self.pool)
                    .set_layouts(&layouts),
            )
        }?;
        self.contour_set = sets[0];
        self.any_sets = [sets[1], sets[2], sets[3]];
        self.tonemap_sets = [sets[4], sets[5]];
        self.bloom_sets = sets[6..].to_vec();

        let read = vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL;
        let depth_read = vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL;
        let hdr = inputs.hdr_view;
        let (a, b) = (self.ping[0].view, self.ping[1].view);
        let d0 = self.bloom[0].view;
        let mut writes: Vec<(vk::DescriptorSet, [(vk::ImageView, vk::ImageLayout); 3])> = vec![
            (
                self.contour_set,
                [
                    (hdr, read),
                    (inputs.depth_view, depth_read),
                    (inputs.normal_view, read),
                ],
            ),
            (self.any_sets[0], [(hdr, read); 3]),
            (self.any_sets[1], [(a, read); 3]),
            (self.any_sets[2], [(b, read); 3]),
            (self.tonemap_sets[0], [(hdr, read), (d0, read), (hdr, read)]),
            (self.tonemap_sets[1], [(a, read), (d0, read), (a, read)]),
        ];
        for (set, level) in self.bloom_sets.iter().zip(&self.bloom) {
            writes.push((*set, [(level.view, read); 3]));
        }
        let infos: Vec<[vk::DescriptorImageInfo; 3]> = writes
            .iter()
            .map(|(_, views)| {
                views.map(|(view, layout)| {
                    vk::DescriptorImageInfo::default()
                        .image_view(view)
                        .image_layout(layout)
                })
            })
            .collect();
        let mut updates = Vec::new();
        for ((set, _), info) in writes.iter().zip(&infos) {
            for (slot, binding) in [0u32, 2, 3].into_iter().enumerate() {
                updates.push(
                    vk::WriteDescriptorSet::default()
                        .dst_set(*set)
                        .dst_binding(binding)
                        .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                        .image_info(std::slice::from_ref(&info[slot])),
                );
            }
        }
        // SAFETY: live sets and views; `infos` outlives the call.
        unsafe { device.update_descriptor_sets(&updates, &[]) };
        Ok(())
    }

    fn destroy(mut self, ctx: &mut Context) {
        // SAFETY: the device is idle or the caller waited; destroying the
        // pool frees its sets.
        unsafe { ctx.device.destroy_descriptor_pool(self.pool, None) };
        for mut img in self.ping.into_iter().chain(self.bloom.drain(..)) {
            img.destroy(ctx);
        }
    }
}

/// Reference CPU model of the tonemap, grading-free path, for tests: exposure,
/// ACES, clamp.
pub fn tonemap_pixel(linear: [f32; 3], exposure: f32) -> [f32; 3] {
    linear.map(|c| aces_tonemap((c * exposure).max(0.0)).clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aces_is_zero_at_zero_and_monotonic() {
        assert_eq!(aces_tonemap(0.0), 0.0);
        let mut prev = aces_tonemap(0.0);
        for i in 1..=1600 {
            let v = aces_tonemap(i as f32 * 0.01);
            assert!(v >= prev, "not monotonic at {}", i as f32 * 0.01);
            prev = v;
        }
    }

    #[test]
    fn aces_stays_below_one_up_to_seven() {
        for i in 0..=700 {
            let x = i as f32 * 0.01;
            assert!(aces_tonemap(x) < 1.0, "T({x}) = {}", aces_tonemap(x));
        }
        // The plan's curve overshoots 1 above x ~ 7.25 (deviation).
        assert!(aces_tonemap(16.0) > 1.0);
        assert!(aces_tonemap(16.0) < 1.04);
        assert_eq!(tonemap_pixel([16.0; 3], 1.0), [1.0; 3]);
    }

    #[test]
    fn settings_default_to_all_on() {
        let s = PostSettings::default();
        assert!(s.contours && s.bloom && s.tonemap && s.grading && s.screen_effects && s.fxaa);
        assert_eq!(
            serde_json_like(&PostSettings::NONE),
            "false false false false false false"
        );
    }

    fn serde_json_like(s: &PostSettings) -> String {
        [
            s.contours,
            s.bloom,
            s.tonemap,
            s.grading,
            s.screen_effects,
            s.fxaa,
        ]
        .map(|b| b.to_string())
        .join(" ")
    }

    #[test]
    fn bloom_chain_halves_down_to_one_pixel() {
        let e = bloom_extents((1280, 720));
        assert_eq!(e[0], (640, 360));
        assert_eq!(e[5], (20, 11));
        let tiny = bloom_extents((3, 1));
        assert!(tiny.iter().all(|&(w, h)| w >= 1 && h >= 1));
    }

    #[test]
    fn params_follow_the_profile() {
        let neon = AmbienceProfile::builtin("neon").unwrap();
        let p = PostParams::from_profile(&neon, false);
        assert_eq!(p.exposure, neon.exposure);
        assert_eq!(p.bloom_intensity, neon.grading.bloom_intensity);
        assert_eq!(p.ripple, 0.0);
        assert!(PostParams::from_profile(&neon, true).ripple > 0.0);
        let d = PostParams::default();
        assert!(!d.effects_active());
        assert_eq!(d.damage_flash, 0.0);
    }
}
