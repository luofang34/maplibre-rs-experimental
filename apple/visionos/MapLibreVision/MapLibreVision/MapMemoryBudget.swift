import Foundation

/// Leaves room for compositor surfaces and allocations already queued by tile workers.
enum MapMemoryBudget {
    static let footprintLimit: UInt64 = 3 * 1_024 * 1_024 * 1_024

    static func available(reported: UInt64, footprint: UInt64) -> UInt64 {
        let headroom = footprint < footprintLimit ? footprintLimit - footprint : 0
        let system = reported == 0 ? UInt64.max : reported
        // Zero means "unknown" at the FFI boundary, so exhausted budgets report one byte.
        return max(1, min(system, headroom))
    }
}
