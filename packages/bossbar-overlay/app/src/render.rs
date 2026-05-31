//! wgpu renderer: draws each boss bar as the same stack of layers Minecraft's
//! `BossHealthOverlay` uses (color background, color progress clipped to the
//! fill, then the optional notch background and notch progress), with the title
//! rendered above in the Minecraft font via glyphon.
//!
//! The renderer is surface-agnostic: it draws into any `TextureView` of a known
//! format, so the same code paints the live overlay window and the headless
//! `--snapshot` PNG used for verification.

use std::collections::HashMap;

use glyphon::{
    Attrs, Buffer, Cache, Color as GColor, Family, FontSystem, Metrics, Resolution, Shaping,
    SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
};
use wgpu::util::DeviceExt;

use crate::assets;
use crate::bars::{BossBar, Color, Notch};

/// Native vanilla sprite dimensions, in unscaled pixels.
const BAR_W: u32 = 182;
const BAR_H: u32 = 5;

/// Default opacity, matching the old CSS `--bar-opacity`: the HUD reads as an
/// overlay by letting the desktop bleed through a little.
const DEFAULT_OPACITY: f32 = 0.85;

/// Vanilla title shadow: one dark pixel offset, scaled, no blur.
const SHADOW: GColor = GColor::rgb(0x3f, 0x3f, 0x3f);

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    pos: [f32; 2],
    uv: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    size: [f32; 2],
    opacity: f32,
    _pad: f32,
}

/// Which preloaded sprite a layer samples.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum TexId {
    ColorBg(Color),
    ColorFill(Color),
    NotchBg(Notch),
    NotchFill(Notch),
}

/// One textured quad to draw, in physical pixels.
struct Quad {
    tex: TexId,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    /// UV span; fill layers narrow `u1` so the sprite is cut off, not squished.
    u1: f32,
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,

    pipeline: wgpu::RenderPipeline,
    globals_buf: wgpu::Buffer,
    globals_bind: wgpu::BindGroup,
    textures: HashMap<TexId, wgpu::BindGroup>,

    font_system: FontSystem,
    swash_cache: SwashCache,
    atlas: TextAtlas,
    text_renderer: TextRenderer,
    viewport: Viewport,

    /// Integer pixel scale of the native 182x5 sprites.
    scale: u32,
    opacity: f32,
}

impl Renderer {
    pub fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        format: wgpu::TextureFormat,
        scale: u32,
    ) -> Self {
        let scale = scale.max(1);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("bossbar.sprite"),
            source: wgpu::ShaderSource::Wgsl(include_str!("sprite.wgsl").into()),
        });

        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bossbar.globals.layout"),
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
            label: Some("bossbar.tex.layout"),
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
            label: Some("bossbar.pipeline.layout"),
            bind_group_layouts: &[&globals_layout, &tex_layout],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("bossbar.pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2],
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
            label: Some("bossbar.globals"),
            size: std::mem::size_of::<Globals>() as wgpu::BufferAddress,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bossbar.globals.bind"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals_buf.as_entire_binding(),
            }],
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("bossbar.sampler"),
            // Nearest keeps the sprites crisp when scaled (CSS image-rendering:
            // pixelated). Magnify only, so no filtering is needed.
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let mut textures = HashMap::new();
        for c in assets::COLORS {
            let (bg, fill) = assets::color_sprites(c);
            textures.insert(
                TexId::ColorBg(c),
                upload_png(&device, &queue, &tex_layout, &sampler, bg),
            );
            textures.insert(
                TexId::ColorFill(c),
                upload_png(&device, &queue, &tex_layout, &sampler, fill),
            );
        }
        for n in assets::NOTCHES {
            let (bg, fill) = assets::notch_sprites(n);
            textures.insert(
                TexId::NotchBg(n),
                upload_png(&device, &queue, &tex_layout, &sampler, bg),
            );
            textures.insert(
                TexId::NotchFill(n),
                upload_png(&device, &queue, &tex_layout, &sampler, fill),
            );
        }

        let mut font_system = FontSystem::new();
        font_system.db_mut().load_font_data(assets::FONT.to_vec());
        let swash_cache = SwashCache::new();
        let cache = Cache::new(&device);
        let viewport = Viewport::new(&device, &cache);
        let mut atlas = TextAtlas::new(&device, &queue, &cache, format);
        let text_renderer =
            TextRenderer::new(&mut atlas, &device, wgpu::MultisampleState::default(), None);

        Self {
            device,
            queue,
            pipeline,
            globals_buf,
            globals_bind,
            textures,
            font_system,
            swash_cache,
            atlas,
            text_renderer,
            viewport,
            scale,
            opacity: DEFAULT_OPACITY,
        }
    }

    /// Draw the given bars into `view`, a `width`x`height` target.
    pub fn render(
        &mut self,
        view: &wgpu::TextureView,
        width: u32,
        height: u32,
        bars: &[BossBar],
    ) -> Result<(), wgpu::SurfaceError> {
        let s = self.scale as f32;
        let bar_w = BAR_W as f32 * s;
        let bar_h = BAR_H as f32 * s;
        let title_px = 8.0 * s;
        let title_gap = 1.0 * s;
        let bar_gap = 4.0 * s;
        let top_pad = 16.0 * s;
        let shadow_off = s;
        let center_x = ((width as f32) - bar_w) * 0.5;

        // Build the sprite quads and lay out the titles in one pass so the two
        // stay in lockstep down the screen.
        let mut quads: Vec<Quad> = Vec::new();
        let mut buffers: Vec<(Buffer, f32, f32, bool)> = Vec::new(); // buffer, left, top, has_text

        let mut y = top_pad;
        for b in bars {
            let has_title = !b.title.is_empty();
            let title_h = if has_title { title_px } else { 0.0 };
            let track_y = y + title_h + if has_title { title_gap } else { 0.0 };

            if has_title {
                let mut buffer =
                    Buffer::new(&mut self.font_system, Metrics::new(title_px, title_px));
                // Center the title within the bar width via cosmic-text's own
                // alignment, so the buffer's left edge sits at the bar's left.
                buffer.set_size(&mut self.font_system, Some(bar_w), Some(title_px * 1.5));
                buffer.set_text(
                    &mut self.font_system,
                    &b.title,
                    &Attrs::new().family(Family::Name(assets::FONT_FAMILY)),
                    Shaping::Advanced,
                    Some(glyphon::cosmic_text::Align::Center),
                );
                buffer.shape_until_scroll(&mut self.font_system, false);
                buffers.push((buffer, center_x, y, true));
            }

            // Color background, then color progress clipped to the fill.
            quads.push(Quad {
                tex: TexId::ColorBg(b.color),
                x: center_x,
                y: track_y,
                w: bar_w,
                h: bar_h,
                u1: 1.0,
            });
            if b.progress > 0.0 {
                quads.push(Quad {
                    tex: TexId::ColorFill(b.color),
                    x: center_x,
                    y: track_y,
                    w: bar_w * b.progress,
                    h: bar_h,
                    u1: b.progress,
                });
            }
            // Optional notch overlay on top, same draw order.
            if let Some(n) = b.overlay.notch() {
                quads.push(Quad {
                    tex: TexId::NotchBg(n),
                    x: center_x,
                    y: track_y,
                    w: bar_w,
                    h: bar_h,
                    u1: 1.0,
                });
                if b.progress > 0.0 {
                    quads.push(Quad {
                        tex: TexId::NotchFill(n),
                        x: center_x,
                        y: track_y,
                        w: bar_w * b.progress,
                        h: bar_h,
                        u1: b.progress,
                    });
                }
            }

            y = track_y + bar_h + bar_gap;
        }

        // Vertex data for every quad, two triangles each.
        let mut verts: Vec<Vertex> = Vec::with_capacity(quads.len() * 6);
        let mut draws: Vec<(TexId, u32)> = Vec::with_capacity(quads.len());
        for q in &quads {
            let base = verts.len() as u32;
            let (x0, y0, x1, y1) = (q.x, q.y, q.x + q.w, q.y + q.h);
            let v = |px, py, u, vv| Vertex {
                pos: [px, py],
                uv: [u, vv],
            };
            verts.extend_from_slice(&[
                v(x0, y0, 0.0, 0.0),
                v(x1, y0, q.u1, 0.0),
                v(x1, y1, q.u1, 1.0),
                v(x0, y0, 0.0, 0.0),
                v(x1, y1, q.u1, 1.0),
                v(x0, y1, 0.0, 1.0),
            ]);
            draws.push((q.tex, base));
        }

        self.queue.write_buffer(
            &self.globals_buf,
            0,
            bytemuck::bytes_of(&Globals {
                size: [width as f32, height as f32],
                opacity: self.opacity,
                _pad: 0.0,
            }),
        );

        let vbuf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("bossbar.verts"),
                contents: if verts.is_empty() {
                    // create_buffer_init rejects empty contents; a single dummy
                    // vertex keeps the buffer valid when there are no bars.
                    bytemuck::cast_slice(&[Vertex {
                        pos: [0.0, 0.0],
                        uv: [0.0, 0.0],
                    }])
                } else {
                    bytemuck::cast_slice(&verts)
                },
                usage: wgpu::BufferUsages::VERTEX,
            });

        // Prepare text. The alpha tracks the bar opacity so titles fade with
        // their bars, matching the old `opacity` on the whole `.bar` element.
        let a = (self.opacity * 255.0) as u8;
        let fg = GColor::rgba(0xff, 0xff, 0xff, a);
        let shadow = GColor::rgba(SHADOW.r(), SHADOW.g(), SHADOW.b(), a);
        let bounds = TextBounds {
            left: 0,
            top: 0,
            right: width as i32,
            bottom: height as i32,
        };
        let mut areas: Vec<TextArea> = Vec::new();
        for (buffer, left, top, has_text) in &buffers {
            if !has_text {
                continue;
            }
            // Shadow first, then the white face one pixel up-left of it.
            areas.push(TextArea {
                buffer,
                left: *left + shadow_off,
                top: *top + shadow_off,
                scale: 1.0,
                bounds,
                default_color: shadow,
                custom_glyphs: &[],
            });
            areas.push(TextArea {
                buffer,
                left: *left,
                top: *top,
                scale: 1.0,
                bounds,
                default_color: fg,
                custom_glyphs: &[],
            });
        }

        self.viewport.update(&self.queue, Resolution { width, height });
        self.text_renderer
            .prepare(
                &self.device,
                &self.queue,
                &mut self.font_system,
                &mut self.atlas,
                &self.viewport,
                areas,
                &mut self.swash_cache,
            )
            .expect("glyphon prepare");

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("bossbar.encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("bossbar.pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // Transparent: the desktop shows through everywhere we
                        // do not paint.
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
                let bind = &self.textures[tex];
                pass.set_bind_group(1, bind, &[]);
                pass.draw(*base..*base + 6, 0..1);
            }

            self.text_renderer
                .render(&self.atlas, &self.viewport, &mut pass)
                .expect("glyphon render");
        }
        self.queue.submit(Some(encoder.finish()));
        self.atlas.trim();
        Ok(())
    }
}

/// Decode a PNG and upload it as an sRGB texture with its own bind group.
fn upload_png(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    bytes: &[u8],
) -> wgpu::BindGroup {
    let img = image::load_from_memory(bytes)
        .expect("decode embedded sprite PNG")
        .to_rgba8();
    let (w, h) = img.dimensions();
    let size = wgpu::Extent3d {
        width: w,
        height: h,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("bossbar.sprite.tex"),
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
        &img,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * w),
            rows_per_image: Some(h),
        },
        size,
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("bossbar.sprite.bind"),
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
