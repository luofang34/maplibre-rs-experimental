import CompositorServices
import Metal

/// Keeps a separate render target for each eye when compositor textures require a copy.
final class MapEyeTargets {
    private var intermediates: [MTLTexture] = []

    func colors(for drawable: LayerRenderer.Drawable, device: MTLDevice, direct: Bool) -> [MTLTexture]? {
        if direct {
            return drawable.views.map { drawable.colorTextures[$0.textureMap.textureIndex] }
        }
        let sizes = drawable.views.map { drawable.colorTextures[$0.textureMap.textureIndex] }
        if intermediates.count != sizes.count || zip(intermediates, sizes).contains(where: {
            $0.width != $1.width || $0.height != $1.height
        }) {
            var targets: [MTLTexture] = []
            for size in sizes {
                let descriptor = MTLTextureDescriptor.texture2DDescriptor(
                    pixelFormat: .bgra8Unorm, width: size.width, height: size.height, mipmapped: false)
                descriptor.storageMode = .private
                descriptor.usage = [.renderTarget]
                guard let texture = device.makeTexture(descriptor: descriptor) else { return nil }
                texture.label = "Map eye \(targets.count)"
                targets.append(texture)
            }
            intermediates = targets
        }
        return intermediates
    }

    func depths(for drawable: LayerRenderer.Drawable) -> [MTLTexture?] {
        drawable.views.map { view in
            let map = view.textureMap
            guard map.textureIndex < drawable.depthTextures.count else { return nil }
            let texture = drawable.depthTextures[map.textureIndex]
            if texture.textureType == .type2D { return texture }
            return texture.makeTextureView(
                pixelFormat: .depth32Float, textureType: .type2D,
                levels: 0..<1, slices: map.sliceIndex..<(map.sliceIndex + 1))
        }
    }

    func copy(to drawable: LayerRenderer.Drawable, commandBuffer: MTLCommandBuffer) -> Bool {
        guard let blit = commandBuffer.makeBlitCommandEncoder() else { return false }
        for (index, view) in drawable.views.enumerated() {
            let source = intermediates[index]
            let map = view.textureMap
            blit.copy(
                from: source, sourceSlice: 0, sourceLevel: 0,
                sourceOrigin: MTLOrigin(x: 0, y: 0, z: 0),
                sourceSize: MTLSize(width: source.width, height: source.height, depth: 1),
                to: drawable.colorTextures[map.textureIndex], destinationSlice: map.sliceIndex,
                destinationLevel: 0, destinationOrigin: MTLOrigin(x: 0, y: 0, z: 0))
        }
        blit.endEncoding()
        return true
    }
}
