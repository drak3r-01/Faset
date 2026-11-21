//! Which upscalers exist, which ones this GPU can run, and the fallback to
//! FSR 1 when one cannot.

use faset_render::{GpuHandles, UpscalePass};

use crate::bridge::UpscaleBridge;
use crate::{Quality, UpscaleError, Upscaler};

/// The upscalers of this crate. Lots 2.11 (FSR 2/3) and 2.12 (DLSS) add
/// their own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum UpscalerKind {
    /// No upscaling: the scene is drawn at the display resolution, or at a
    /// free render scale and stretched with a bilinear filter.
    Native,
    /// Temporal anti-aliasing at the display resolution.
    Taa,
    /// FidelityFX Super Resolution 1: purely spatial, runs everywhere.
    Fsr1,
}

impl UpscalerKind {
    /// Every upscaler of this crate, FSR 1 last.
    pub const ALL: [Self; 3] = [Self::Native, Self::Taa, Self::Fsr1];

    /// The upscaler every failure falls back to.
    pub const FALLBACK: Self = Self::Fsr1;

    pub fn name(self) -> &'static str {
        match self {
            Self::Native => "Native",
            Self::Taa => "TAA",
            Self::Fsr1 => "FSR 1",
        }
    }

    /// True when the result depends on previous frames: the scene must be
    /// jittered and the FXAA fallback is skipped.
    pub fn is_temporal(self) -> bool {
        matches!(self, Self::Taa)
    }

    /// True when the free render scale applies ([`crate::effective_quality`]).
    pub fn supports_free_scale(self) -> bool {
        matches!(self, Self::Native | Self::Fsr1)
    }
}

/// The kinds to try, in order, for a request: the request, then FSR 1.
pub fn fallback_order(requested: UpscalerKind) -> Vec<UpscalerKind> {
    let mut order = vec![requested];
    if requested != UpscalerKind::FALLBACK {
        order.push(UpscalerKind::FALLBACK);
    }
    order
}

/// One upscaler a [`Registry`] can build. `C` is what it needs to probe and
/// create itself: [`GpuHandles`] for the real ones.
pub trait Backend<C> {
    fn kind(&self) -> UpscalerKind;

    /// Checks that this upscaler can run, without keeping anything.
    fn probe(&self, ctx: &C) -> Result<(), UpscaleError>;

    /// Builds the upscaler.
    fn create(&self, ctx: &C) -> Result<Box<dyn Upscaler>, UpscaleError>;
}

/// The outcome of [`Registry::create`].
pub struct Created {
    /// What was asked for.
    pub requested: UpscalerKind,
    /// What was built: `requested`, or FSR 1 after a failure.
    pub kind: UpscalerKind,
    pub upscaler: Box<dyn Upscaler>,
}

impl Created {
    /// True when the requested upscaler failed and FSR 1 stands in.
    pub fn fell_back(&self) -> bool {
        self.kind != self.requested
    }
}

/// The set of upscalers, detected at startup.
pub struct Registry<C> {
    backends: Vec<Box<dyn Backend<C>>>,
}

impl<C> Default for Registry<C> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C> Registry<C> {
    pub fn new() -> Self {
        Self {
            backends: Vec::new(),
        }
    }

    /// Adds an upscaler; a later one of the same kind replaces the earlier.
    pub fn register(&mut self, backend: Box<dyn Backend<C>>) {
        self.backends.retain(|b| b.kind() != backend.kind());
        self.backends.push(backend);
    }

    /// The upscalers that can run here, in registration order. A failing
    /// probe is logged and leaves the mode out.
    pub fn available(&self, ctx: &C) -> Vec<UpscalerKind> {
        self.backends
            .iter()
            .filter(|b| match b.probe(ctx) {
                Ok(()) => true,
                Err(e) => {
                    tracing::warn!("{} is unavailable: {e}", b.kind().name());
                    false
                }
            })
            .map(|b| b.kind())
            .collect()
    }

    /// Builds `requested`. When it is missing, fails its probe or fails to
    /// build, FSR 1 is built instead; when that fails too, the error is
    /// [`UpscaleError::NoUpscaler`].
    pub fn create(&self, requested: UpscalerKind, ctx: &C) -> Result<Created, UpscaleError> {
        for kind in fallback_order(requested) {
            let Some(backend) = self.backends.iter().find(|b| b.kind() == kind) else {
                tracing::warn!("{} is not registered", kind.name());
                continue;
            };
            match backend.probe(ctx).and_then(|()| backend.create(ctx)) {
                Ok(upscaler) => {
                    if kind != requested {
                        tracing::warn!(
                            "{} failed, falling back to {}",
                            requested.name(),
                            kind.name()
                        );
                    }
                    return Ok(Created {
                        requested,
                        kind,
                        upscaler,
                    });
                }
                Err(e) => tracing::warn!("{} failed: {e}", kind.name()),
            }
        }
        Err(UpscaleError::NoUpscaler)
    }
}

impl Registry<GpuHandles> {
    /// Native, TAA and FSR 1, all built on the renderer's device.
    pub fn builtin() -> Self {
        let mut registry = Self::new();
        for kind in UpscalerKind::ALL {
            registry.register(Box::new(Builtin(kind)));
        }
        registry
    }

    /// [`Registry::create`], wrapped for [`faset_render::Renderer::set_upscaler`].
    /// Returns the kind that was built, which is FSR 1 after a failure.
    pub fn create_pass(
        &self,
        requested: UpscalerKind,
        quality: Quality,
        gpu: &GpuHandles,
    ) -> Result<(UpscalerKind, Box<dyn UpscalePass>), UpscaleError> {
        let created = self.create(requested, gpu)?;
        let kind = created.kind;
        let bridge = UpscaleBridge::new(kind, created.upscaler, quality);
        Ok((kind, Box::new(bridge)))
    }
}

/// Builds one of the crate's own upscalers.
struct Builtin(UpscalerKind);

impl Backend<GpuHandles> for Builtin {
    fn kind(&self) -> UpscalerKind {
        self.0
    }

    fn probe(&self, gpu: &GpuHandles) -> Result<(), UpscaleError> {
        crate::gpu::check_support(gpu)
    }

    fn create(&self, gpu: &GpuHandles) -> Result<Box<dyn Upscaler>, UpscaleError> {
        Ok(match self.0 {
            UpscalerKind::Native => Box::new(crate::native::Native::new(gpu)?),
            UpscalerKind::Taa => Box::new(crate::taa::Taa::new(gpu)?),
            UpscalerKind::Fsr1 => Box::new(crate::fsr1::Fsr1::new(gpu)?),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use faset_render::{FrameCtx, ImageHandle, UpscaleInputs};

    use super::*;
    use crate::Extent2D;

    /// Does nothing; records nothing.
    struct Stub(UpscalerKind);

    impl Upscaler for Stub {
        fn render_extent(&self, display: Extent2D, _quality: Quality) -> Extent2D {
            display
        }
        fn jitter(&self, _frame_index: u32) -> faset_render::glam::Vec2 {
            faset_render::glam::Vec2::ZERO
        }
        fn execute(&mut self, _ctx: &mut FrameCtx<'_>, inputs: &UpscaleInputs) -> ImageHandle {
            let _ = self.0;
            inputs.colour
        }
    }

    /// Fake backend: `probe_ok` and `create_ok` choose how it fails; every
    /// attempt is logged in the context.
    struct Fake {
        kind: UpscalerKind,
        probe_ok: bool,
        create_ok: bool,
    }

    type Log = RefCell<Vec<String>>;

    impl Backend<Log> for Fake {
        fn kind(&self) -> UpscalerKind {
            self.kind
        }
        fn probe(&self, log: &Log) -> Result<(), UpscaleError> {
            log.borrow_mut().push(format!("probe {:?}", self.kind));
            if self.probe_ok {
                Ok(())
            } else {
                Err(UpscaleError::Unavailable {
                    kind: self.kind,
                    reason: "no".into(),
                })
            }
        }
        fn create(&self, log: &Log) -> Result<Box<dyn Upscaler>, UpscaleError> {
            log.borrow_mut().push(format!("create {:?}", self.kind));
            if self.create_ok {
                Ok(Box::new(Stub(self.kind)))
            } else {
                Err(UpscaleError::Unavailable {
                    kind: self.kind,
                    reason: "broken".into(),
                })
            }
        }
    }

    fn registry(taa: (bool, bool), fsr1: (bool, bool)) -> Registry<Log> {
        let mut r = Registry::new();
        for (kind, (probe_ok, create_ok)) in [
            (UpscalerKind::Native, (true, true)),
            (UpscalerKind::Taa, taa),
            (UpscalerKind::Fsr1, fsr1),
        ] {
            r.register(Box::new(Fake {
                kind,
                probe_ok,
                create_ok,
            }));
        }
        r
    }

    #[test]
    fn a_working_upscaler_is_built_as_requested() {
        let log = Log::default();
        let made = registry((true, true), (true, true))
            .create(UpscalerKind::Taa, &log)
            .unwrap();
        assert_eq!(made.kind, UpscalerKind::Taa);
        assert!(!made.fell_back());
        assert_eq!(*log.borrow(), ["probe Taa", "create Taa"]);
    }

    #[test]
    fn a_failing_upscaler_falls_back_to_fsr1() {
        for taa in [(false, true), (true, false), (false, false)] {
            let log = Log::default();
            let made = registry(taa, (true, true))
                .create(UpscalerKind::Taa, &log)
                .unwrap();
            assert_eq!(made.requested, UpscalerKind::Taa);
            assert_eq!(made.kind, UpscalerKind::Fsr1, "{taa:?}");
            assert!(made.fell_back());
        }
    }

    #[test]
    fn nothing_is_built_when_the_fallback_fails_too() {
        let log = Log::default();
        let result = registry((false, false), (true, false)).create(UpscalerKind::Taa, &log);
        assert!(matches!(result, Err(UpscaleError::NoUpscaler)));
        // FSR 1 itself is tried once only.
        let result = registry((true, true), (false, true)).create(UpscalerKind::Fsr1, &log);
        assert!(matches!(result, Err(UpscaleError::NoUpscaler)));
    }

    #[test]
    fn an_unregistered_kind_falls_back() {
        let mut r: Registry<Log> = Registry::new();
        r.register(Box::new(Fake {
            kind: UpscalerKind::Fsr1,
            probe_ok: true,
            create_ok: true,
        }));
        let made = r.create(UpscalerKind::Native, &Log::default()).unwrap();
        assert_eq!(made.kind, UpscalerKind::Fsr1);
    }

    #[test]
    fn available_lists_the_modes_that_pass_their_probe() {
        let log = Log::default();
        let r = registry((false, true), (true, true));
        assert_eq!(
            r.available(&log),
            [UpscalerKind::Native, UpscalerKind::Fsr1]
        );
    }

    #[test]
    fn fallback_order_ends_with_fsr1() {
        assert_eq!(
            fallback_order(UpscalerKind::Taa),
            [UpscalerKind::Taa, UpscalerKind::Fsr1]
        );
        assert_eq!(fallback_order(UpscalerKind::Fsr1), [UpscalerKind::Fsr1]);
    }

    #[test]
    fn kinds_describe_themselves() {
        assert!(UpscalerKind::Taa.is_temporal());
        assert!(!UpscalerKind::Fsr1.is_temporal());
        assert!(UpscalerKind::Native.supports_free_scale());
        assert!(!UpscalerKind::Taa.supports_free_scale());
    }
}
