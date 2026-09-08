import CompositorServices
import Metal

/// Publishes colour and depth together, keeping one complete stereo frame for recovery.
final class MapEyeTargets {
    private var drawingColors: [MTLTexture] = []
    private var drawingDepths: [MTLTexture] = []
    private var presentedColors: [MTLTexture] = []
    private var presentedDepths: [MTLTexture] = []

    var hasPresentedFrame: Bool { !presentedColors.isEmpty }

    func colors(for drawable: LayerRenderer.Drawable, device: MTLDevice) -> [MTLTexture]? {
        let sizes = drawable.views.map { drawable.colorTextures[$0.textureMap.textureIndex] }
        if !presentedColors.isEmpty, !matches(presentedColors, sizes) {
            presentedColors = []; presentedDepths = []
        }
        if !matches(drawingColors, sizes) {
            guard let colors = allocate(sizes, .bgra8Unorm, device),
                  let depths = allocate(sizes, .depth32Float, device) else { return nil }
            drawingColors = colors; drawingDepths = depths
        }
        return drawingColors
    }

    func depths() -> [MTLTexture?] { drawingDepths.map { $0 } }

    func publish() {
        swap(&drawingColors, &presentedColors)
        swap(&drawingDepths, &presentedDepths)
    }

    func copy(to drawable: LayerRenderer.Drawable, commandBuffer: MTLCommandBuffer, opaque: Bool) {
        guard presentedColors.count == drawable.views.count,
              let blit = commandBuffer.makeBlitCommandEncoder() else {
            clear(drawable, commandBuffer, opaque)
            return
        }
        for (index, view) in drawable.views.enumerated() {
            let map = view.textureMap
            copy(presentedColors[index], to: drawable.colorTextures[map.textureIndex], slice: map.sliceIndex, blit: blit)
            if map.textureIndex < drawable.depthTextures.count {
                copy(presentedDepths[index], to: drawable.depthTextures[map.textureIndex], slice: map.sliceIndex, blit: blit)
            }
        }
        blit.endEncoding()
    }

    private func matches(_ textures: [MTLTexture], _ sizes: [MTLTexture]) -> Bool {
        textures.count == sizes.count && zip(textures, sizes).allSatisfy {
            $0.width == $1.width && $0.height == $1.height
        }
    }

    private func allocate(_ sizes: [MTLTexture], _ format: MTLPixelFormat, _ device: MTLDevice) -> [MTLTexture]? {
        var textures: [MTLTexture] = []
        for size in sizes {
            let descriptor = MTLTextureDescriptor.texture2DDescriptor(
                pixelFormat: format, width: size.width, height: size.height, mipmapped: false)
            descriptor.storageMode = .private
            descriptor.usage = [.renderTarget]
            guard let texture = device.makeTexture(descriptor: descriptor) else { return nil }
            texture.label = "Map stereo frame"
            textures.append(texture)
        }
        return textures
    }

    private func copy(_ source: MTLTexture, to destination: MTLTexture, slice: Int, blit: MTLBlitCommandEncoder) {
        blit.copy(from: source, sourceSlice: 0, sourceLevel: 0,
                  sourceOrigin: MTLOrigin(x: 0, y: 0, z: 0),
                  sourceSize: MTLSize(width: source.width, height: source.height, depth: 1),
                  to: destination, destinationSlice: slice, destinationLevel: 0,
                  destinationOrigin: MTLOrigin(x: 0, y: 0, z: 0))
    }

    private func clear(_ drawable: LayerRenderer.Drawable, _ commandBuffer: MTLCommandBuffer, _ opaque: Bool) {
        for view in drawable.views {
            let map = view.textureMap
            let pass = MTLRenderPassDescriptor()
            pass.colorAttachments[0].texture = drawable.colorTextures[map.textureIndex]
            pass.colorAttachments[0].slice = map.sliceIndex
            pass.colorAttachments[0].loadAction = .clear
            pass.colorAttachments[0].storeAction = .store
            pass.colorAttachments[0].clearColor = MTLClearColor(red: 0.02, green: 0.025, blue: 0.035, alpha: opaque ? 1 : 0)
            if map.textureIndex < drawable.depthTextures.count {
                pass.depthAttachment.texture = drawable.depthTextures[map.textureIndex]
                pass.depthAttachment.slice = map.sliceIndex
                pass.depthAttachment.loadAction = .clear
                pass.depthAttachment.storeAction = .store
                pass.depthAttachment.clearDepth = opaque ? 1e-8 : 0
            }
            commandBuffer.makeRenderCommandEncoder(descriptor: pass)?.endEncoding()
        }
    }
}
