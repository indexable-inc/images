//! The shared wgpu engine: one textured-quad pipeline plus a texture registry,
//! with the Minecraft bitmap font baked in so text is just more quads.
//!
//! Vertices arrive in physical pixels with a top-left origin; the vertex stage
//! converts to clip space using the framebuffer size, so all layout math stays in
//! pixel units on the CPU (matching how Minecraft blits its GUI sprites). Both
//! overlays build a `Vec<Quad>` and hand it to [`Gpu::draw`] (live window) or
//! [`crate::snapshot`] (headless PNG).

use crate::bitmap_font::{self, BitmapFont};

use wgpu::util::DeviceExt;

/// Vanilla title-shadow grey: one scaled pixel down-right of the glyph.
pub const SHADOW: [f32; 4] = [
    0x3f as f32 / 255.0,
    0x3f as f32 / 255.0,
    0x3f as f32 / 255.0,
    1.0,
];

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    pos: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    size: [f32; 2],
    _pad: [f32; 2],
}

/// Handle to a texture registered in a [`Gpu`]. Cheap to copy; indexes the GPU's
/// bind-group table.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TexHandle(u32);

/// One textured quad, in physical pixels. `uv` is `(u0, v0, u1, v1)`; pass
/// `u0 > u1` to mirror horizontally (used for the book's right-hand page).
/// `color` is a straight-alpha RGBA tint multiplied into the sampled texel:
/// white shows the texture unchanged, and the 1x1 white texture from
/// [`Gpu::white`] turns the tint into a flat fill.
#[derive(Clone, Copy)]
pub struct Quad {
    pub tex: TexHandle,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub uv: [f32; 4],
    pub color: [f32; 4],
}

impl Quad {
    /// A quad sampling the whole texture at the given rect.
    pub fn new(tex: TexHandle, x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) -> Self {
        Self {
            tex,
            x,
            y,
            w,
            h,
            uv: [0.0, 0.0, 1.0, 1.0],
            color,
        }
    }

    /// A quad sampling a sub-rect `(u0, v0, u1, v1)` of its texture.
    pub fn sub(
        tex: TexHandle,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        uv: [f32; 4],
        color: [f32; 4],
    ) -> Self {
        Self {
            tex,
            x,
            y,
            w,
            h,
            uv,
            color,
        }
    }
}

/// The device, the textured-quad pipeline, the registered textures, and the
/// bitmap font. Surface-agnostic: it draws into any `TextureView` of the format
/// it was built with, so the same engine paints a live window and a snapshot PNG.
pub struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    globals_buf: wgpu::Buffer,
    globals_bind: wgpu::BindGroup,
    tex_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    textures: Vec<wgpu::BindGroup>,
    font_tex: TexHandle,
    white_tex: TexHandle,
}

impl Gpu {
    /// Build the engine on an existing device/queue for `format`. Registers the
    /// embedded Minecraft font and a 1x1 white texture up front.
    pub fn new(device: wgpu::Device, queue: wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("overlay.sprite"),
            source: wgpu::ShaderSource::Wgsl(include_str!("sprite.wgsl").into()),
        });

        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("overlay.globals.layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let tex_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("overlay.tex.layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("overlay.pipeline.layout"),
            bind_group_layouts: &[&globals_layout, &tex_layout],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("overlay.pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("overlay.globals"),
            size: std::mem::size_of::<Globals>() as wgpu::BufferAddress,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("overlay.globals.bind"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals_buf.as_entire_binding(),
            }],
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("overlay.sampler"),
            // Nearest keeps the pixel art crisp when scaled (image-rendering:
            // pixelated). Magnify only, so no min filtering is needed.
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        // Register the well-known textures before constructing self: a 1x1 white
        // pixel, then the Minecraft font sheet. The glyph metrics come from the
        // shared CPU face ([`bitmap_font::shared`]), which retains only the widths,
        // so the sheet is decoded again here for the texture's pixels. Both read
        // the same embedded `ASCII_PNG`, so metrics and texture never disagree;
        // this decode is intentionally separate, not a candidate to fold into
        // `shared()` (doing so would couple GPU bring-up to the lazy metrics init).
        let mut textures = Vec::new();
        let white_tex = {
            let bind =
                upload_texture(&device, &queue, &tex_layout, &sampler, &[0xff, 0xff, 0xff, 0xff], 1, 1);
            let h = TexHandle(textures.len() as u32);
            textures.push(bind);
            h
        };
        let ascii = image::load_from_memory(bitmap_font::ASCII_PNG)
            .expect("decode embedded ascii.png")
            .to_rgba8();
        let (fw, fh) = ascii.dimensions();
        let font_tex = {
            let bind = upload_texture(&device, &queue, &tex_layout, &sampler, &ascii, fw, fh);
            let h = TexHandle(textures.len() as u32);
            textures.push(bind);
            h
        };

        Self {
            device,
            queue,
            pipeline,
            globals_buf,
            globals_bind,
            tex_layout,
            sampler,
            textures,
            font_tex,
            white_tex,
        }
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// The Minecraft bitmap font, for measuring text and laying out wraps. Same
    /// shared face window sizing measures with, so on-CPU layout and the drawn
    /// glyphs always agree.
    pub fn font(&self) -> &'static BitmapFont {
        bitmap_font::shared()
    }

    /// A 1x1 opaque-white texture; tint a quad sampling it to paint a flat fill.
    pub fn white(&self) -> TexHandle {
        self.white_tex
    }

    /// Decode a PNG and register it, returning its handle.
    pub fn register_png(&mut self, bytes: &[u8]) -> TexHandle {
        let img = image::load_from_memory(bytes)
            .expect("decode PNG")
            .to_rgba8();
        let (w, h) = img.dimensions();
        self.register_rgba(&img, w, h)
    }

    /// Register raw sRGB RGBA8 pixels as a nearest-sampled texture.
    pub fn register_rgba(&mut self, rgba: &[u8], width: u32, height: u32) -> TexHandle {
        let bind = upload_texture(
            &self.device,
            &self.queue,
            &self.tex_layout,
            &self.sampler,
            rgba,
            width,
            height,
        );
        let handle = TexHandle(self.textures.len() as u32);
        self.textures.push(bind);
        handle
    }

    /// Lay out `text` as glyph quads at `(x, y)` top-left, `scale` px per source
    /// pixel, tinted `color`. Returns the advance width drawn.
    pub fn text(&self, text: &str, x: f32, y: f32, scale: f32, color: [f32; 4], out: &mut Vec<Quad>) -> f32 {
        let font = bitmap_font::shared();
        let cell = BitmapFont::cell_px() * scale;
        let mut pen = x;
        for c in text.chars() {
            if let Some(uv) = font.glyph_uv(c) {
                out.push(Quad::sub(self.font_tex, pen, y, cell, cell, uv, color));
            }
            pen += font.advance(c, scale);
        }
        pen - x
    }

    /// `text`, drawn with a one-pixel shadow first (vanilla style). Returns the
    /// foreground advance width.
    pub fn text_shadow(
        &self,
        text: &str,
        x: f32,
        y: f32,
        scale: f32,
        color: [f32; 4],
        shadow: [f32; 4],
        out: &mut Vec<Quad>,
    ) -> f32 {
        self.text(text, x + scale, y + scale, scale, shadow, out);
        self.text(text, x, y, scale, color, out)
    }

    /// Measure `text`'s advance width at `scale` without drawing.
    pub fn measure(&self, text: &str, scale: f32) -> f32 {
        bitmap_font::shared().measure(text, scale)
    }

    /// Paint `quads` into `view` over a transparent clear. Quads draw in order, so
    /// later quads layer over earlier ones.
    pub fn draw(
        &self,
        view: &wgpu::TextureView,
        width: u32,
        height: u32,
        quads: &[Quad],
    ) -> Result<(), wgpu::SurfaceError> {
        let mut verts: Vec<Vertex> = Vec::with_capacity(quads.len() * 6);
        let mut draws: Vec<(TexHandle, u32)> = Vec::with_capacity(quads.len());
        for q in quads {
            let base = verts.len() as u32;
            let (x0, y0, x1, y1) = (q.x, q.y, q.x + q.w, q.y + q.h);
            let (u0, v0, u1, v1) = (q.uv[0], q.uv[1], q.uv[2], q.uv[3]);
            let color = q.color;
            let v = |px, py, u, vv| Vertex {
                pos: [px, py],
                uv: [u, vv],
                color,
            };
            verts.extend_from_slice(&[
                v(x0, y0, u0, v0),
                v(x1, y0, u1, v0),
                v(x1, y1, u1, v1),
                v(x0, y0, u0, v0),
                v(x1, y1, u1, v1),
                v(x0, y1, u0, v1),
            ]);
            draws.push((q.tex, base));
        }

        self.queue.write_buffer(
            &self.globals_buf,
            0,
            bytemuck::bytes_of(&Globals {
                size: [width as f32, height as f32],
                _pad: [0.0, 0.0],
            }),
        );

        let vbuf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("overlay.verts"),
                contents: if verts.is_empty() {
                    // create_buffer_init rejects empty contents; one dummy vertex
                    // keeps the buffer valid when there is nothing to draw.
                    bytemuck::cast_slice(&[Vertex {
                        pos: [0.0, 0.0],
                        uv: [0.0, 0.0],
                        color: [0.0, 0.0, 0.0, 0.0],
                    }])
                } else {
                    bytemuck::cast_slice(&verts)
                },
                usage: wgpu::BufferUsages::VERTEX,
            });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("overlay.encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("overlay.pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // Transparent: the desktop shows through everywhere unpainted.
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.globals_bind, &[]);
            pass.set_vertex_buffer(0, vbuf.slice(..));
            for (tex, base) in &draws {
                pass.set_bind_group(1, &self.textures[tex.0 as usize], &[]);
                pass.draw(*base..*base + 6, 0..1);
            }
        }
        self.queue.submit(Some(encoder.finish()));
        Ok(())
    }
}

/// Upload `rgba` (sRGB RGBA8, `width`x`height`) as a nearest-sampled texture and
/// return its bind group. A free function so it can run before a [`Gpu`] exists:
/// the white pixel and the font sheet are uploaded while building one.
fn upload_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    rgba: &[u8],
    width: u32,
    height: u32,
) -> wgpu::BindGroup {
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("overlay.tex"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * width),
            rows_per_image: Some(height),
        },
        size,
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("overlay.tex.bind"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}
