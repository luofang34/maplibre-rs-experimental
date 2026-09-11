import CoreGraphics

enum FlightHUDContrast {
    static func apply(to context: CGContext) {
        // Rasterize contrast with telemetry, so each eye reuses it at display cadence.
        context.setShadow(offset: .zero, blur: 2,
                          color: CGColor(gray: 0, alpha: 0.95))
    }
}
