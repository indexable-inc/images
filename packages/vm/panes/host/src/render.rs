//! Shared Metal state: one device/queue/pipeline for every window.
//!
//! Frame path (v1, CPU upload): tiles are decompressed and `replaceRegion`ed
//! into one of two per-window `MTLTexture`s (double-buffered in `window`:
//! `replaceRegion` does not synchronize against GPU access, so uploads must
//! never touch the texture a still-executing present is sampling), then a
//! fullscreen-triangle render pass samples that texture into the drawable. A
//! render pass (not a blit) because `CAMetalLayer.framebufferOnly = true`
//! allows drawables only as color render targets (Apple: framebufferOnly
//! "allows the system to apply optimizations"; blit destinations would
//! require turning it off), and because sampling stretches the stale texture
//! for free during live resize while the guest catches up.

use core::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::NSString;
use objc2_metal::{
    MTLCommandBuffer, MTLCommandEncoder, MTLCommandQueue, MTLCreateSystemDefaultDevice, MTLDevice,
    MTLDrawable, MTLLibrary, MTLLoadAction, MTLOrigin, MTLPixelFormat, MTLPrimitiveType,
    MTLRegion, MTLRenderCommandEncoder, MTLRenderPassDescriptor, MTLRenderPipelineDescriptor,
    MTLRenderPipelineState, MTLSize, MTLStoreAction, MTLTexture, MTLTextureDescriptor,
    MTLTextureUsage,
};
use objc2_quartz_core::CAMetalDrawable;
use panes_protocol::Rect;

/// Vertex-id-only fullscreen triangle plus a plain sampler; the whole
/// presentation shader surface. Compiled at startup with
/// `newLibraryWithSource:` so there is no offline metallib build step.
const SHADER_SOURCE: &str = r"
#include <metal_stdlib>
using namespace metal;

struct VOut {
    float4 position [[position]];
    float2 uv;
};

// One triangle covering clip space ((-1,-1) (3,-1) (-1,3)); uv flips y
// because Metal NDC is y-up while the surface buffer is y-down.
vertex VOut panes_vertex(uint vid [[vertex_id]]) {
    float2 corner = float2((vid << 1) & 2, vid & 2);
    VOut out;
    out.position = float4(corner * 2.0 - 1.0, 0.0, 1.0);
    out.uv = float2(corner.x, 1.0 - corner.y);
    return out;
}

fragment float4 panes_fragment(VOut in [[stage_in]],
                               texture2d<float> tex [[texture(0)]]) {
    constexpr sampler s(mag_filter::linear, min_filter::linear);
    return tex.sample(s, in.uv);
}
";

pub struct Renderer {
    pub device: Retained<ProtocolObject<dyn MTLDevice>>,
    queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    pipeline: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
}

impl Renderer {
    pub fn new() -> Result<Self, String> {
        let device =
            MTLCreateSystemDefaultDevice().ok_or_else(|| "no Metal device".to_string())?;
        let queue = device
            .newCommandQueue()
            .ok_or_else(|| "newCommandQueue failed".to_string())?;
        let library = device
            .newLibraryWithSource_options_error(&NSString::from_str(SHADER_SOURCE), None)
            .map_err(|error| format!("shader compile failed: {error}"))?;
        let vertex = library
            .newFunctionWithName(&NSString::from_str("panes_vertex"))
            .ok_or_else(|| "panes_vertex missing".to_string())?;
        let fragment = library
            .newFunctionWithName(&NSString::from_str("panes_fragment"))
            .ok_or_else(|| "panes_fragment missing".to_string())?;

        let descriptor = MTLRenderPipelineDescriptor::new();
        descriptor.setVertexFunction(Some(&vertex));
        descriptor.setFragmentFunction(Some(&fragment));
        // Index 0 is the only attachment; the subscript is not bounds-checked
        // by the binding, hence unsafe.
        unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) }
            .setPixelFormat(MTLPixelFormat::BGRA8Unorm);
        let pipeline = device
            .newRenderPipelineStateWithDescriptor_error(&descriptor)
            .map_err(|error| format!("pipeline creation failed: {error}"))?;

        Ok(Self { device, queue, pipeline })
    }

    /// One per-window surface texture (a slot of the double buffer).
    /// `ShaderRead` only: the CPU writes it via `replaceRegion`, the GPU
    /// samples it.
    pub fn make_texture(
        &self,
        width: u32,
        height: u32,
    ) -> Option<Retained<ProtocolObject<dyn MTLTexture>>> {
        let descriptor = unsafe {
            MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
                MTLPixelFormat::BGRA8Unorm,
                width as usize,
                height as usize,
                false,
            )
        };
        descriptor.setUsage(MTLTextureUsage::ShaderRead);
        self.device.newTextureWithDescriptor(&descriptor)
    }

    /// Upload one decoded tile. `bytes` must be exactly `rect.w * rect.h * 4`
    /// tightly-packed BGRA rows; the caller validates rect bounds against the
    /// texture before calling.
    pub fn upload(texture: &ProtocolObject<dyn MTLTexture>, rect: Rect, bytes: &[u8]) {
        debug_assert_eq!(bytes.len(), rect.w as usize * rect.h as usize * 4);
        let region = MTLRegion {
            origin: MTLOrigin { x: rect.x as usize, y: rect.y as usize, z: 0 },
            size: MTLSize { width: rect.w as usize, height: rect.h as usize, depth: 1 },
        };
        let Some(ptr) = NonNull::new(bytes.as_ptr().cast_mut()) else {
            return;
        };
        // SAFETY: `bytes` covers rect.w * rect.h tightly-packed rows
        // (asserted above) and the caller keeps rect inside the texture.
        unsafe {
            texture.replaceRegion_mipmapLevel_withBytes_bytesPerRow(
                region,
                0,
                ptr.cast(),
                rect.w as usize * 4,
            );
        }
    }

    /// Sample `source` into `drawable` and present. Returns the committed
    /// command buffer so the caller can tell when the GPU is done reading
    /// `source` (`replaceRegion` into it before then would race the read).
    /// Returns None when Metal gave no command buffer/encoder (device loss);
    /// the caller keeps the frame pending and retries next tick.
    pub fn draw(
        &self,
        source: &ProtocolObject<dyn MTLTexture>,
        drawable: &ProtocolObject<dyn CAMetalDrawable>,
        presents_with_transaction: bool,
    ) -> Option<Retained<ProtocolObject<dyn MTLCommandBuffer>>> {
        let commands = self.queue.commandBuffer()?;
        let pass = MTLRenderPassDescriptor::renderPassDescriptor();
        let attachment = unsafe { pass.colorAttachments().objectAtIndexedSubscript(0) };
        let target = drawable.texture();
        attachment.setTexture(Some(&target));
        // Clear (not Load): the triangle covers everything, so Load would
        // only force a needless restore of undefined drawable contents.
        attachment.setLoadAction(MTLLoadAction::Clear);
        attachment.setStoreAction(MTLStoreAction::Store);
        let encoder = commands.renderCommandEncoderWithDescriptor(&pass)?;
        encoder.setRenderPipelineState(&self.pipeline);
        unsafe {
            encoder.setFragmentTexture_atIndex(Some(source), 0);
            encoder.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::Triangle, 0, 3);
        }
        encoder.endEncoding();

        if presents_with_transaction {
            // Live resize: commit, wait until the GPU scheduled the work,
            // then present inside the current CATransaction so the new frame
            // moves in lockstep with the window frame (Apple, CAMetalLayer
            // presentsWithTransaction docs / WWDC20 "Optimize Metal apps and
            // games with GPU counters" resize guidance).
            commands.commit();
            commands.waitUntilScheduled();
            drawable.present();
        } else {
            commands.presentDrawable(ProtocolObject::from_ref(drawable));
            commands.commit();
        }
        Some(commands)
    }
}
