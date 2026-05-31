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

/// How much a fully-hovered bar grows, before breathing. A small, deliberate
/// scale-up (on top of going opaque) so the hover is unmistakable.
const HOVER_SCALE: f32 = 1.06;

/// Breathing amplitude: a hovered bar gently scales +/- this fraction around its
/// grown size on a slow sine, so it reads as alive rather than frozen.
const BREATHE_AMP: f32 = 0.02;

/// Largest scale a bar can reach (grown + breathing in). Each window reserves
/// this much headroom so the bar grows and breathes in place without the window
/// resizing or shifting.
const MAX_SCALE: f32 = HOVER_SCALE * (1.0 + BREATHE_AMP);

/// Vanilla title shadow: one dark pixel offset, scaled, no blur.
const SHADOW: GColor = GColor::rgb(0x3f, 0x3f, 0x3f);

/// Description pop-down panel, in native (unscaled) pixels. Everything is
/// multiplied by the integer sprite `scale`, so the panel stays pixel-crisp and
/// proportional to the bars at any display scale.
mod panel {
    /// Body font size and line advance (leading). The face matches the title's
    /// Minecraft font; the extra leading gives wrapped paragraphs room.
    pub const FONT: f32 = 8.0;
    pub const LINE: f32 = 10.0;
    /// Inner text padding and the flat one-pixel border frame.
    pub const PAD: f32 = 5.0;
    pub const BORDER: f32 = 1.0;
    /// Gap between the bar's reserved (hover-headroom) area and the panel top.
    pub const GAP: f32 = 3.0;
    /// Flat dark-slate fill, kept slightly translucent so the desktop bleeds
    /// through like the bars. Straight (non-premultiplied) RGBA in 0..=1.
    pub const BG: [f32; 4] = [0x12 as f32 / 255.0, 0x0f as f32 / 255.0, 0x1a as f32 / 255.0, 0.92];
    /// Border opacity; its RGB comes from the bar color's accent.
    pub const BORDER_ALPHA: f32 = 0.95;
}

/// Straight-alpha RGBA in 0..=1 from an 8-bit RGB triple and an alpha.
fn rgba(rgb: [u8; 3], a: f32) -> [f32; 4] {
    [
        rgb[0] as f32 / 255.0,
        rgb[1] as f32 / 255.0,
        rgb[2] as f32 / 255.0,
        a,
    ]
}

/// Smoothstep ramp: 0 below `lo`, 1 above `hi`, eased between. Lets the panel
/// text fade in only after the box has begun to unfold.
fn ramp(x: f32, lo: f32, hi: f32) -> f32 {
    let t = ((x - lo) / (hi - lo)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    pos: [f32; 2],
    uv: [f32; 2],
    /// Straight-alpha RGBA tint multiplied into the sampled texel. Bars pass
    /// white so they show unchanged (the alpha lets a hovered bar paint solid
    /// over the translucent rest); the panel samples a 1x1 white texture, so the
    /// tint *is* its flat fill or border color.
    color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    size: [f32; 2],
    _pad: [f32; 2],
}

/// Which preloaded sprite a layer samples. `Solid` is a 1x1 white texture used
/// by the description panel's flat fill and border, tinted via the vertex color.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum TexId {
    ColorBg(Color),
    ColorFill(Color),
    NotchBg(Notch),
    NotchFill(Notch),
    Solid,
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
    /// Straight-alpha RGBA tint for this quad; see [`Vertex::color`].
    color: [f32; 4],
}

/// Physical-pixel geometry of one laid-out bar, in the target's local space.
/// Produced by [`Renderer::layout`] (multi-bar, for the `--snapshot` PNG) and by
/// [`Renderer::render_one`] (a single bar filling its own window).
#[derive(Clone, Copy)]
struct BarBox {
    left: f32,
    title_top: f32,
    track_y: f32,
    bar_w: f32,
    bar_h: f32,
    title_px: f32,
    has_title: bool,
}

/// Physical-pixel geometry of the description pop-down panel, plus its current
/// reveal. The box unfolds downward as `reveal` goes 0..1; the text fades in via
/// `text_alpha` slightly behind it. Width matches the bar; produced by
/// [`Renderer::render_one`] (hover-driven) and [`Renderer::render`] (fully open,
/// for the `--snapshot` PNG).
#[derive(Clone, Copy)]
struct PanelBox {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    border: f32,
    pad: f32,
    font_px: f32,
    line_px: f32,
    /// Vertical unfold of the box in `0..=1`, anchored at the top edge.
    reveal: f32,
    /// Text fade in `0..=1`, lagged behind `reveal` so the box opens first.
    text_alpha: f32,
    /// Border RGB (the bar color's accent), 0..=255; the fill is [`panel::BG`].
    border_rgb: [u8; 3],
}

/// One bar to paint: which bar, its box in target-local pixels, its opacity, and
/// an optional description panel unfolding beneath it.
struct DrawItem<'a> {
    bar: &'a BossBar,
    geom: BarBox,
    alpha: f32,
    panel: Option<PanelBox>,
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,

    pipeline: wgpu::RenderPipeline,
    globals_buf: wgpu::Buffer,
    globals_bind: wgpu::BindGroup,
    textures: HashMap<TexId, wgpu::BindGroup>,

    font_system: FontSystem,
    /// The embedded font's real family name, read back from its `name` table
    /// rather than hardcoded. cosmic-text matches families by name and silently
    /// substitutes a system font on a miss, so a stale literal would render a
    /// non-Minecraft font with no error. Deriving it keeps the selector in lock
    /// step with whatever `MinecraftDefault-Regular.ttf` actually reports.
    font_family: String,
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
        // A single white texel the panel quads sample so the vertex color tints
        // them directly: white * color = color (a flat fill or border).
        textures.insert(
            TexId::Solid,
            upload_white_pixel(&device, &queue, &tex_layout, &sampler),
        );

        // Load *only* the embedded Minecraft font into an otherwise empty
        // database. cosmic-text's default `FontSystem::new` also loads every
        // installed system font and falls back to one when a family name does
        // not match, which is exactly how the title silently rendered in a
        // generic system font. With a single-font db the title is the Minecraft
        // font or nothing, never a wrong substitute.
        let mut db = glyphon::cosmic_text::fontdb::Database::new();
        db.load_font_data(assets::FONT.to_vec());
        let font_family = db
            .faces()
            .next()
            .and_then(|face| face.families.first())
            .map(|(name, _)| name.clone())
            .expect("embedded Minecraft font is missing a family name");
        let font_system = FontSystem::new_with_locale_and_db("en-US".to_string(), db);
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
            font_family,
            swash_cache,
            atlas,
            text_renderer,
            viewport,
            scale,
            opacity: DEFAULT_OPACITY,
        }
    }

    /// Physical-pixel geometry of every bar auto-stacked down the top-center
    /// column, in draw order. Used by the multi-bar `--snapshot` render; the
    /// live overlay gives each bar its own window and uses `render_one`. `panels`
    /// is the per-bar panel size (parallel to `bars`); a bar with a panel
    /// reserves `gap + panel_h` extra vertical space so the next bar clears it.
    fn layout(
        &self,
        bars: &[BossBar],
        width: f32,
        panels: &[Option<(f32, f32)>],
        gap: f32,
    ) -> Vec<BarBox> {
        let s = self.scale as f32;
        let bar_w = BAR_W as f32 * s;
        let bar_h = BAR_H as f32 * s;
        let title_px = 8.0 * s;
        let title_gap = 1.0 * s;
        let bar_gap = 4.0 * s;
        let top_pad = 16.0 * s;
        let center_x = (width - bar_w) * 0.5;

        let mut boxes = Vec::with_capacity(bars.len());
        let mut y = top_pad;
        for (b, panel) in bars.iter().zip(panels) {
            let has_title = !b.title.is_empty();
            let title_h = if has_title { title_px } else { 0.0 };
            let track_y = y + title_h + if has_title { title_gap } else { 0.0 };
            boxes.push(BarBox {
                left: center_x,
                title_top: y,
                track_y,
                bar_w,
                bar_h,
                title_px,
                has_title,
            });
            // Advance past whatever this bar drew last: its panel bottom when it
            // has one (sitting `gap` below the bar), otherwise the bar bottom.
            let bottom = match panel {
                Some((_, panel_h)) => track_y + bar_h + gap + panel_h,
                None => track_y + bar_h,
            };
            y = bottom + bar_gap;
        }
        boxes
    }

    /// Low-level draw: paint each item (a bar, its box, its opacity) into
    /// `view`. Shared by the multi-bar snapshot render and the per-window render.
    fn draw(
        &mut self,
        view: &wgpu::TextureView,
        width: u32,
        height: u32,
        items: &[DrawItem<'_>],
    ) -> Result<(), wgpu::SurfaceError> {
        // A shaped run of text plus where, how opaque, and the rect to clip it
        // to. Titles span the whole target; description lines clip to the
        // unfolding panel so they reveal with the box.
        struct Text {
            buffer: Buffer,
            left: f32,
            top: f32,
            alpha: f32,
            clip: TextBounds,
        }

        let shadow_off = self.scale as f32;
        let full_bounds = TextBounds {
            left: 0,
            top: 0,
            right: width as i32,
            bottom: height as i32,
        };

        let mut quads: Vec<Quad> = Vec::new();
        let mut texts: Vec<Text> = Vec::new();

        for item in items {
            let b = item.bar;
            let bx = item.geom;
            let alpha = item.alpha;
            // Bars sample real sprites, so the tint is white with the bar's
            // opacity; only the alpha channel matters for them.
            let tint = [1.0, 1.0, 1.0, alpha];

            if bx.has_title {
                let mut buffer =
                    Buffer::new(&mut self.font_system, Metrics::new(bx.title_px, bx.title_px));
                // Center the title within the bar width via cosmic-text's own
                // alignment, so the buffer's left edge sits at the bar's left.
                buffer.set_size(&mut self.font_system, Some(bx.bar_w), Some(bx.title_px * 1.5));
                buffer.set_text(
                    &mut self.font_system,
                    &b.title,
                    &Attrs::new().family(Family::Name(&self.font_family)),
                    Shaping::Advanced,
                    Some(glyphon::cosmic_text::Align::Center),
                );
                buffer.shape_until_scroll(&mut self.font_system, false);
                texts.push(Text {
                    buffer,
                    left: bx.left,
                    top: bx.title_top,
                    alpha,
                    clip: full_bounds,
                });
            }

            // Color background, then color progress clipped to the fill.
            quads.push(Quad {
                tex: TexId::ColorBg(b.color),
                x: bx.left,
                y: bx.track_y,
                w: bx.bar_w,
                h: bx.bar_h,
                u1: 1.0,
                color: tint,
            });
            if b.progress > 0.0 {
                quads.push(Quad {
                    tex: TexId::ColorFill(b.color),
                    x: bx.left,
                    y: bx.track_y,
                    w: bx.bar_w * b.progress,
                    h: bx.bar_h,
                    u1: b.progress,
                    color: tint,
                });
            }
            // Optional notch overlay on top, same draw order.
            if let Some(n) = b.overlay.notch() {
                quads.push(Quad {
                    tex: TexId::NotchBg(n),
                    x: bx.left,
                    y: bx.track_y,
                    w: bx.bar_w,
                    h: bx.bar_h,
                    u1: 1.0,
                    color: tint,
                });
                if b.progress > 0.0 {
                    quads.push(Quad {
                        tex: TexId::NotchFill(n),
                        x: bx.left,
                        y: bx.track_y,
                        w: bx.bar_w * b.progress,
                        h: bx.bar_h,
                        u1: b.progress,
                        color: tint,
                    });
                }
            }

            // Description pop-down: a flat bordered box that unfolds downward,
            // with the wrapped paragraph fading in behind it.
            if let Some(p) = item.panel.filter(|p| p.reveal > 0.001) {
                let revealed_h = (p.h * p.reveal).max(0.0);
                // Border frame first, then the fill inset by the border. While
                // unfolding (revealed_h < 2*border) only the accent strip shows,
                // so the box reads as opening from a thin line.
                quads.push(Quad {
                    tex: TexId::Solid,
                    x: p.x,
                    y: p.y,
                    w: p.w,
                    h: revealed_h,
                    u1: 1.0,
                    color: rgba(p.border_rgb, panel::BORDER_ALPHA),
                });
                let inner_x = p.x + p.border;
                let inner_w = (p.w - 2.0 * p.border).max(0.0);
                let inner_y = p.y + p.border;
                let inner_h = (revealed_h - 2.0 * p.border).max(0.0);
                quads.push(Quad {
                    tex: TexId::Solid,
                    x: inner_x,
                    y: inner_y,
                    w: inner_w,
                    h: inner_h,
                    u1: 1.0,
                    color: panel::BG,
                });

                if p.text_alpha > 0.001 && !b.description.trim().is_empty() {
                    let text_w = (p.w - 2.0 * (p.border + p.pad)).max(1.0);
                    let mut buffer =
                        Buffer::new(&mut self.font_system, Metrics::new(p.font_px, p.line_px));
                    buffer.set_size(&mut self.font_system, Some(text_w), None);
                    buffer.set_text(
                        &mut self.font_system,
                        &b.description,
                        &Attrs::new().family(Family::Name(&self.font_family)),
                        Shaping::Advanced,
                        None,
                    );
                    buffer.shape_until_scroll(&mut self.font_system, false);
                    // Clip to the revealed inner area so lines wipe in with the
                    // box rather than popping in fully formed.
                    let clip = TextBounds {
                        left: inner_x as i32,
                        top: inner_y as i32,
                        right: (inner_x + inner_w) as i32,
                        bottom: (p.y + revealed_h - p.border) as i32,
                    };
                    texts.push(Text {
                        buffer,
                        left: inner_x + p.pad,
                        top: inner_y + p.pad,
                        alpha: p.text_alpha,
                        clip,
                    });
                }
            }
        }

        // Vertex data for every quad, two triangles each.
        let mut verts: Vec<Vertex> = Vec::with_capacity(quads.len() * 6);
        let mut draws: Vec<(TexId, u32)> = Vec::with_capacity(quads.len());
        for q in &quads {
            let base = verts.len() as u32;
            let (x0, y0, x1, y1) = (q.x, q.y, q.x + q.w, q.y + q.h);
            let color = q.color;
            let v = |px, py, u, vv| Vertex {
                pos: [px, py],
                uv: [u, vv],
                color,
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
                _pad: [0.0, 0.0],
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
                        color: [0.0, 0.0, 0.0, 0.0],
                    }])
                } else {
                    bytemuck::cast_slice(&verts)
                },
                usage: wgpu::BufferUsages::VERTEX,
            });

        // Prepare text. Each run's alpha tracks its own bar (a hovered bar goes
        // opaque, title and description included) and clips to its own rect.
        let mut areas: Vec<TextArea> = Vec::new();
        for text in &texts {
            let a = (text.alpha * 255.0) as u8;
            let fg = GColor::rgba(0xff, 0xff, 0xff, a);
            let shadow = GColor::rgba(SHADOW.r(), SHADOW.g(), SHADOW.b(), a);
            // Shadow first, then the white face one pixel up-left of it.
            areas.push(TextArea {
                buffer: &text.buffer,
                left: text.left + shadow_off,
                top: text.top + shadow_off,
                scale: 1.0,
                bounds: text.clip,
                default_color: shadow,
                custom_glyphs: &[],
            });
            areas.push(TextArea {
                buffer: &text.buffer,
                left: text.left,
                top: text.top,
                scale: 1.0,
                bounds: text.clip,
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

    /// Render all bars auto-stacked into one target (the `--snapshot` PNG path).
    /// `highlight` is the id of a bar to paint opaque. Every bar with a
    /// description shows its panel fully open, so the snapshot verifies the
    /// pop-down the live overlay only reveals on hover.
    pub fn render(
        &mut self,
        view: &wgpu::TextureView,
        width: u32,
        height: u32,
        bars: &[BossBar],
        highlight: Option<i64>,
    ) -> Result<(), wgpu::SurfaceError> {
        let opacity = self.opacity;
        let (border, pad, font_px, line_px, gap) = self.panel_metrics();
        let sizes: Vec<Option<(f32, f32)>> =
            bars.iter().map(|b| self.panel_size(&b.description)).collect();
        let boxes = self.layout(bars, width as f32, &sizes, gap);
        let items: Vec<DrawItem<'_>> = bars
            .iter()
            .zip(boxes)
            .zip(&sizes)
            .map(|((bar, geom), size)| {
                let panel = size.map(|(panel_w, panel_h)| PanelBox {
                    x: ((width as f32 - panel_w) * 0.5).max(0.0),
                    y: geom.track_y + geom.bar_h + gap,
                    w: panel_w,
                    h: panel_h,
                    border,
                    pad,
                    font_px,
                    line_px,
                    reveal: 1.0,
                    text_alpha: 1.0,
                    border_rgb: bar.color.accent_rgb(),
                });
                DrawItem {
                    bar,
                    geom,
                    alpha: if Some(bar.id) == highlight { 1.0 } else { opacity },
                    panel,
                }
            })
            .collect();
        self.draw(view, width, height, &items)
    }

    /// Render a single bar centered in its own window. `hover` is the eased
    /// hover amount (0 = resting, 1 = fully hovered); `breathe` is a sine in
    /// `-1..1` for the idle breathing. Together they grow the bar by up to
    /// [`MAX_SCALE`] and fade it to opaque; at `hover == 0` the bar is base size
    /// and translucent and the hover headroom is transparent margin. The window
    /// size must come from [`bar_window_px`] so the grown bar fits without
    /// resizing.
    ///
    /// When the bar has a description and the window has grown tall enough (the
    /// overlay enlarges it on hover, see [`Renderer::expanded_window_px`]), a
    /// description panel unfolds beneath the bar, revealing with `hover`.
    pub fn render_one(
        &mut self,
        view: &wgpu::TextureView,
        width: u32,
        height: u32,
        bar: &BossBar,
        hover: f32,
        breathe: f32,
    ) -> Result<(), wgpu::SurfaceError> {
        let hover = hover.clamp(0.0, 1.0);
        // Grow toward HOVER_SCALE with hover, then breathe around that; the
        // breathe fades in with hover so a resting bar is perfectly still.
        let grow = 1.0 + (HOVER_SCALE - 1.0) * hover;
        let scale_mul = grow * (1.0 + BREATHE_AMP * breathe * hover);
        let alpha = self.opacity + (1.0 - self.opacity) * hover;
        let s = self.scale as f32 * scale_mul;
        let shadow = self.scale as f32;
        let has_title = !bar.title.is_empty();
        let title_px = 8.0 * s;
        let title_h = if has_title { title_px } else { 0.0 };
        let title_gap = if has_title { 1.0 * s } else { 0.0 };
        let bar_w = BAR_W as f32 * s;
        let bar_h = BAR_H as f32 * s;

        // The bar lives in the top region: the collapsed window size, which holds
        // it plus its grow/breathe headroom. Any extra window height below that
        // is the panel's drop area, so the bar stays put as the panel unfolds.
        let collapsed_h = bar_window_px(self.scale, has_title).1 as f32;
        let top_region_h = collapsed_h.min(height as f32);

        // Center the content (plus its shadow offset) in the top region so growth
        // on hover expands evenly from the middle rather than shifting a corner.
        let content_w = bar_w + shadow;
        let content_h = title_h + title_gap + bar_h + shadow;
        let left = ((width as f32 - content_w) * 0.5).max(0.0);
        let top = ((top_region_h - content_h) * 0.5).max(0.0);

        let geom = BarBox {
            left,
            title_top: top,
            track_y: top + title_h + title_gap,
            bar_w,
            bar_h,
            title_px,
            has_title,
        };

        // Only build the panel when the window was actually grown for it; a
        // collapsed window (height == top_region_h) has no room and skips it.
        let panel = if height as f32 > collapsed_h + 0.5 {
            self.panel_size(&bar.description).map(|(panel_w, panel_h)| {
                let (border, pad, font_px, line_px, gap) = self.panel_metrics();
                PanelBox {
                    x: ((width as f32 - panel_w) * 0.5).max(0.0),
                    y: collapsed_h + gap,
                    w: panel_w,
                    h: panel_h,
                    border,
                    pad,
                    font_px,
                    line_px,
                    reveal: hover,
                    text_alpha: ramp(hover, 0.35, 1.0),
                    border_rgb: bar.color.accent_rgb(),
                }
            })
        } else {
            None
        };

        let items = [DrawItem {
            bar,
            geom,
            alpha,
            panel,
        }];
        self.draw(view, width, height, &items)
    }

    /// Panel metrics in physical pixels at the current scale:
    /// `(border, pad, font, line, gap)`.
    fn panel_metrics(&self) -> (f32, f32, f32, f32, f32) {
        let s = self.scale as f32;
        (
            panel::BORDER * s,
            panel::PAD * s,
            panel::FONT * s,
            panel::LINE * s,
            panel::GAP * s,
        )
    }

    /// Physical-pixel size `(width, height)` of the description panel for
    /// `description` at the current scale: width matches the bar, height fits the
    /// wrapped, padded text. `None` for an empty description (no panel).
    fn panel_size(&mut self, description: &str) -> Option<(f32, f32)> {
        if description.trim().is_empty() {
            return None;
        }
        let (border, pad, font_px, line_px, _gap) = self.panel_metrics();
        let panel_w = BAR_W as f32 * self.scale as f32;
        let text_w = (panel_w - 2.0 * (border + pad)).max(1.0);
        let lines = self.measure_lines(description, text_w, font_px, line_px).max(1);
        let panel_h = 2.0 * (border + pad) + lines as f32 * line_px;
        Some((panel_w, panel_h))
    }

    /// Count the wrapped visual lines `description` shapes to at `text_w`. Drives
    /// panel height, so it uses the same font, size, and wrap width as the draw.
    fn measure_lines(&mut self, description: &str, text_w: f32, font_px: f32, line_px: f32) -> usize {
        let mut buffer = Buffer::new(&mut self.font_system, Metrics::new(font_px, line_px));
        buffer.set_size(&mut self.font_system, Some(text_w), None);
        buffer.set_text(
            &mut self.font_system,
            description,
            &Attrs::new().family(Family::Name(&self.font_family)),
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(&mut self.font_system, false);
        buffer.layout_runs().count()
    }

    /// Physical-pixel window size for `bar` with its hover panel open: the
    /// collapsed bar window grown downward by the gap plus the panel. Returns the
    /// collapsed size when the bar has no description. The overlay grows the
    /// window to this on hover so the panel has room to unfold.
    pub fn expanded_window_px(&mut self, bar: &BossBar) -> (u32, u32) {
        let (cw, ch) = bar_window_px(self.scale, !bar.title.is_empty());
        match self.panel_size(&bar.description) {
            Some((panel_w, panel_h)) => {
                let gap = panel::GAP * self.scale as f32;
                (
                    cw.max(panel_w.ceil() as u32),
                    ch + (gap + panel_h).ceil() as u32,
                )
            }
            None => (cw, ch),
        }
    }
}

/// Physical-pixel size of the window that holds one bar at `scale` (base scale
/// times the display factor), including the [`HOVER_SCALE`] headroom so a
/// hovered bar grows in place. `has_title` adds the title row; plus a
/// one-pixel-scaled shadow margin.
pub fn bar_window_px(scale: u32, has_title: bool) -> (u32, u32) {
    let s = scale.max(1) as f32 * MAX_SCALE;
    let bar_w = BAR_W as f32 * s;
    let bar_h = BAR_H as f32 * s;
    let title = if has_title { 8.0 * s + 1.0 * s } else { 0.0 };
    let shadow = scale.max(1) as f32;
    ((bar_w + shadow).ceil() as u32, (title + bar_h + shadow).ceil() as u32)
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

/// Upload a 1x1 opaque-white sRGB texel with its own bind group. The panel quads
/// sample it so the per-vertex color becomes a flat fill: white * color = color.
fn upload_white_pixel(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    let size = wgpu::Extent3d {
        width: 1,
        height: 1,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("bossbar.solid.tex"),
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
        &[0xff, 0xff, 0xff, 0xff],
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4),
            rows_per_image: Some(1),
        },
        size,
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("bossbar.solid.bind"),
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
