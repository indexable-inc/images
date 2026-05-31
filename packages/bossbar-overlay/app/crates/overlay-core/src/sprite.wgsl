// Textured-quad pipeline for the boss bar sprite layers. Vertices arrive in
// physical pixels with a top-left origin; the vertex stage converts to clip
// space using the framebuffer size, so layout math stays in pixel units on the
// CPU (matching how Minecraft blits its 182x5 sprites).

struct Globals {
    size: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs(@location(0) px: vec2<f32>, @location(1) uv: vec2<f32>, @location(2) color: vec4<f32>) -> VsOut {
    var out: VsOut;
    let ndc = vec2<f32>(
        px.x / globals.size.x * 2.0 - 1.0,
        1.0 - px.y / globals.size.y * 2.0,
    );
    out.pos = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = uv;
    out.color = color;
    return out;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let c = textureSample(tex, samp, in.uv);
    // Straight alpha out; the pipeline's ALPHA_BLENDING state composites it into
    // the premultiplied framebuffer, the same blend glyphon uses for text, so
    // sprites and titles layer consistently over the transparent desktop. The
    // per-vertex color tints the sampled texel: sprites pass white (so they show
    // unchanged, with the tint's alpha letting a hovered bar paint solid), while
    // the description panel samples a 1x1 white texture so the tint *is* the
    // flat fill or border color.
    return vec4<f32>(c.rgb * in.color.rgb, c.a * in.color.a);
}
