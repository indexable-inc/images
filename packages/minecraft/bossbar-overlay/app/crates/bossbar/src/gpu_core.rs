//! GPU resources shared by the portable and Wayland layer-shell backends.

use std::collections::HashMap;

use overlay_core::{Gpu, TexHandle, wgpu, window as ocwin};

use crate::scene::{self, BarTextures};
use crate::theme;

/// GPU state shared by every boss-bar surface.
pub(crate) struct GpuCore {
    pub(crate) gpu: Gpu,
    pub(crate) textures: BarTextures,
    pub(crate) format: wgpu::TextureFormat,
    pub(crate) alpha_mode: wgpu::CompositeAlphaMode,
    icon_cache: HashMap<String, Option<TexHandle>>,
    theme_cache: theme::ThemeCache,
}

impl GpuCore {
    pub(crate) fn new(instance: &wgpu::Instance, surface: &wgpu::Surface<'_>) -> Self {
        let (adapter, device, queue) = ocwin::request_adapter_device(instance, surface);
        let caps = surface.get_capabilities(&adapter);
        let format = ocwin::srgb_format(&caps);
        let alpha_mode = ocwin::transparent_alpha_mode(&caps);
        let mut gpu = Gpu::new(device, queue, format);
        let textures = scene::register(&mut gpu);

        Self {
            gpu,
            textures,
            format,
            alpha_mode,
            icon_cache: HashMap::new(),
            theme_cache: theme::ThemeCache::new(),
        }
    }

    /// Resolve an icon path to its texture, loading and caching on first use.
    /// Read failures remain retryable because the writer may create the file
    /// after the bar first appears; successful or failed decodes are stable.
    pub(crate) fn icon(&mut self, path: &str) -> Option<TexHandle> {
        if path.is_empty() {
            return None;
        }
        if let Some(cached) = self.icon_cache.get(path) {
            return *cached;
        }
        let Ok(bytes) = std::fs::read(path) else {
            return None;
        };
        let handle = self.gpu.register_image_scaled(&bytes, scene::ICON_MAX_PX);
        self.icon_cache.insert(path.to_string(), handle);
        handle
    }

    /// Resolve a theme name to its uploaded texture set.
    pub(crate) fn theme(&mut self, name: &str) -> Option<theme::ThemeSprites> {
        self.theme_cache.resolve(&mut self.gpu, name)
    }
}
