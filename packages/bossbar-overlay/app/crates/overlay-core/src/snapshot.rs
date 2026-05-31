//! Headless render of an overlay scene to a transparent PNG: the observability
//! hook. It runs the same [`Gpu`] the live window uses against an offscreen
//! texture, so an always-on-top transparent window (awkward to screenshot) is
//! verifiable pixel-for-pixel from a file.

use std::path::Path;

use crate::gpu::{Gpu, Quad};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// Render a scene into a `width`x`height` transparent PNG at `out`. `build`
/// receives a fresh [`Gpu`] to register its textures and returns the quads to
/// draw, exactly as the live overlay would build them.
pub fn render_to_png<F>(
    width: u32,
    height: u32,
    build: F,
    out: &Path,
) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnOnce(&mut Gpu) -> Vec<Quad>,
{
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::default(),
        compatible_surface: None,
        force_fallback_adapter: false,
    }))?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("overlay.snapshot.device"),
        ..Default::default()
    }))?;

    let mut gpu = Gpu::new(device, queue, FORMAT);
    let quads = build(&mut gpu);

    let target = gpu.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("overlay.snapshot.target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    gpu.draw(&view, width, height, &quads)
        .map_err(|e| format!("render: {e:?}"))?;

    // Copy the texture into a readback buffer with the 256-byte row alignment
    // wgpu requires, then strip the padding before writing the PNG.
    let bytes_per_pixel = 4u32;
    let unpadded = width * bytes_per_pixel;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded = unpadded.div_ceil(align) * align;
    let buffer = gpu.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some("overlay.snapshot.readback"),
        size: (padded * height) as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let mut encoder = gpu
        .device()
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("overlay.snapshot.copy"),
        });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    gpu.queue().submit(Some(encoder.finish()));

    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    gpu.device().poll(wgpu::PollType::wait_indefinitely())?;
    rx.recv()??;

    let data = slice.get_mapped_range();
    let mut pixels = Vec::with_capacity((unpadded * height) as usize);
    for row in 0..height {
        let start = (row * padded) as usize;
        pixels.extend_from_slice(&data[start..start + unpadded as usize]);
    }
    drop(data);
    buffer.unmap();

    image::save_buffer(out, &pixels, width, height, image::ExtendedColorType::Rgba8)?;
    Ok(())
}
