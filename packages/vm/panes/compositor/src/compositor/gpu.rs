//! GPU (linux-dmabuf) readback path, compiled only with the `gpu` feature.
//!
//! GL/Vulkan clients cannot render into `wl_shm`, they produce dmabufs. To get
//! their pixels onto the wire the compositor imports each dmabuf as a GLES
//! texture and reads it back to CPU memory. The context is created
//! *surfaceless* on an `EGLDevice` (`EGL_MESA_platform_surfaceless` /
//! `EGL_EXT_platform_device`): no DRM master, no KMS, no output, so it works
//! on a bare render node like virtio-gpu's /dev/dri/renderD128 inside the
//! VM. libEGL is dlopen'd at runtime (smithay's `backend_egl` uses
//! `libloading`), and `Gpu::try_new` degrades gracefully, so the same binary
//! (built with this feature, the default) runs shm-only on a GPU-less
//! machine: no render node means no linux-dmabuf global is advertised.

use anyhow::Context as _;
// `Dmabuf::format` comes from the allocator `Buffer` trait.
use smithay::backend::allocator::Buffer as _;
use smithay::backend::allocator::Fourcc;
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::allocator::format::FormatSet;
use smithay::backend::egl::{EGLContext, EGLDevice, EGLDisplay};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::{ExportMem as _, ImportDma as _, Texture as _, TextureMapping as _};
use smithay::utils::{Point, Rectangle, Size};

pub struct Gpu {
    renderer: GlesRenderer,
}

/// Cheap pre-flight for `/dev/dri/renderD*`: EGL device enumeration is only
/// attempted when the kernel actually exposes a render node.
fn has_render_node() -> bool {
    std::fs::read_dir("/dev/dri").is_ok_and(|entries| {
        entries
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().starts_with("renderD"))
    })
}

/// A dmabuf's pixels read back to CPU memory in the wire's packed BGRA.
pub struct GpuFrame {
    pub bgra: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

impl Gpu {
    pub fn try_new() -> anyhow::Result<Self> {
        // smithay's EGL loader panics (an `expect` in
        // src/backend/egl/ffi.rs) when libEGL.so.1 cannot be dlopen'd,
        // instead of returning Err. A GPU-less guest must degrade to
        // shm-only rather than crash at startup, so probe for a render node
        // first and absorb any unwind from the EGL stack.
        if !has_render_node() {
            anyhow::bail!("no /dev/dri render node; shm-only");
        }
        // Init owns everything it creates and nothing escapes on unwind, so
        // observing a caught panic here cannot expose broken state.
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(Self::init))
            .unwrap_or_else(|_| Err(anyhow::anyhow!("EGL stack panicked during init")))
    }

    fn init() -> anyhow::Result<Self> {
        let device = EGLDevice::enumerate()
            .context("enumerate EGL devices (libEGL present?)")?
            .find(|device| device.try_get_render_node().ok().flatten().is_some())
            .context("no EGL device with a DRM render node (/dev/dri absent?)")?;
        // Safety: exactly one EGLDisplay is created per device handle for the
        // process lifetime (Gpu is constructed once in App::new).
        let display = unsafe { EGLDisplay::new(device) }.context("create EGLDisplay")?;
        let context = EGLContext::new(&display).context("create EGLContext")?;
        // Safety: the context is not made current on any other thread; the
        // renderer owns it from here and all calls happen on the event-loop
        // thread.
        let renderer = unsafe { GlesRenderer::new(context) }.context("create GLES renderer")?;
        Ok(Self { renderer })
    }

    /// Formats to advertise on the linux-dmabuf global: the renderer's
    /// importable set restricted to the 32-bpp RGBA family `readback` can
    /// actually serve. The renderer imports far more (10/16-bit, YUV), but a
    /// client that picked one of those would hit a readback error and show a
    /// permanently black window — dmabuf clients don't fall back to shm once
    /// they've bound the global.
    pub fn formats(&self) -> FormatSet {
        const SERVABLE: [Fourcc; 8] = [
            Fourcc::Argb8888,
            Fourcc::Xrgb8888,
            Fourcc::Abgr8888,
            Fourcc::Xbgr8888,
            Fourcc::Rgba8888,
            Fourcc::Rgbx8888,
            Fourcc::Bgra8888,
            Fourcc::Bgrx8888,
        ];
        self.renderer
            .dmabuf_formats()
            .iter()
            .filter(|format| SERVABLE.contains(&format.code))
            .copied()
            .collect()
    }

    /// Validation import for `DmabufHandler::dmabuf_imported`.
    pub fn import(&mut self, dmabuf: &Dmabuf) -> bool {
        self.renderer.import_dmabuf(dmabuf, None).is_ok()
    }

    /// dmabuf -> GLES texture -> CPU copy. Argb8888 fourcc is BGRA bytes in
    /// little-endian memory, i.e. the wire format, so no swizzle pass is
    /// needed after `map_texture`.
    pub fn readback(&mut self, dmabuf: &Dmabuf) -> anyhow::Result<GpuFrame> {
        let texture = self
            .renderer
            .import_dmabuf(dmabuf, None)
            .context("import dmabuf")?;
        let width = texture.width();
        let height = texture.height();
        // smithay calls the buffer-pixel coordinate space marker `Buffer`.
        let size: Size<i32, smithay::utils::Buffer> = (
            i32::try_from(width).context("texture width exceeds i32")?,
            i32::try_from(height).context("texture height exceeds i32")?,
        )
            .into();
        let mapping = self
            .renderer
            .copy_texture(
                &texture,
                Rectangle::new(Point::from((0, 0)), size),
                Fourcc::Argb8888,
            )
            .context("copy texture")?;
        let bytes = self.renderer.map_texture(&mapping).context("map texture")?;
        let mut bgra = bytes.to_vec();
        // Everything downstream (row repack, force-opaque, FrameStore::commit)
        // assumes tightly packed width*height*4; commit's debug_assert is
        // stripped in release, so this is the only guard between a padded or
        // truncated readback and a corrupt frame on the wire.
        let stride = usize::try_from(width).context("texture width exceeds usize")? * 4;
        let expected = stride * usize::try_from(height).context("texture height exceeds usize")?;
        anyhow::ensure!(
            stride > 0 && bgra.len() == expected,
            "readback of {} bytes is not tightly packed {width}x{height}x4",
            bgra.len()
        );
        // GLES readback is bottom-up (smithay's `GlesMapping::flipped()` is
        // unconditionally true) while the wire is top-down; ship it as-is and
        // every dmabuf window renders upside down on the host. Repack rows in
        // reverse, keyed on `flipped()` so a future non-flipped mapping stays
        // correct.
        if mapping.flipped() {
            let mut top_down = Vec::with_capacity(bgra.len());
            for row in bgra.rchunks_exact(stride) {
                top_down.extend_from_slice(row);
            }
            bgra = top_down;
        }
        // The copy above is Argb8888, but an alpha-less source format leaves
        // the A byte undefined (commonly 0). The wire is premultiplied BGRA,
        // so A=0 would composite the whole window invisible on the host;
        // force opaque, mirroring the shm path's `force_opaque` for XRGB.
        if matches!(
            dmabuf.format().code,
            Fourcc::Xrgb8888 | Fourcc::Xbgr8888 | Fourcc::Rgbx8888 | Fourcc::Bgrx8888
        ) {
            for px in bgra.chunks_exact_mut(4) {
                px[3] = 0xFF;
            }
        }
        Ok(GpuFrame {
            bgra,
            width,
            height,
        })
    }
}
