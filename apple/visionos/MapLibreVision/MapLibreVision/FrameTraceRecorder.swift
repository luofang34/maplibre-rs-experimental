import Foundation
import Metal
import QuartzCore

/// Adds what the host measures of each frame to the map's frame timeline, and appends the
/// timeline to `frame-trace.jsonl` in the app's caches every few seconds, one JSON export
/// per line, for a profiling run to collect.
///
/// CPU spans are wall time on the render thread, including encoding GPU commands; the only
/// GPU span is the host command buffer's own execution, from its GPU timestamps. Completion
/// handlers run on another thread, so their results wait here until the next frame records
/// them, since only the render thread may touch the map.
final class FrameTraceRecorder {
    private struct Completion {
        let frame: UInt64
        let gpu: Double
        let deadline: Double
        let completed: Double
    }

    private let lock = NSLock()
    private var completions: [Completion] = []
    private var frames = 0
    private var lastExport = CACurrentMediaTime()
    private let exportURL = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)
        .first?.appendingPathComponent("frame-trace.jsonl")

    static let capacity: UInt32 = 1200
    static let exportEverySeconds = 10.0
    static let deviceSampleEvery = 30

    func enable(_ map: OpaquePointer) {
        maplibre_visionos_trace_enable(map, FrameTraceRecorder.capacity)
    }

    /// The number the map gave the frame it drew last, which the host's spans go under.
    func frame(_ map: OpaquePointer) -> UInt64 {
        maplibre_visionos_last_frame(map)
    }

    func cpu(_ map: OpaquePointer, frame: UInt64, _ name: String, seconds: Double) {
        span(map, frame: frame, name, gpu: false, seconds: seconds)
    }

    /// Records, once `commandBuffer` completes, its GPU execution as `copy-overlay` and when
    /// the frame finished against the compositor's deadline, both from `start`.
    func watch(_ commandBuffer: MTLCommandBuffer, frame: UInt64, start: Double, deadline: Double) {
        commandBuffer.addCompletedHandler { [weak self] buffer in
            let completion = Completion(
                frame: frame, gpu: max(0, buffer.gpuEndTime - buffer.gpuStartTime),
                deadline: deadline, completed: max(0, buffer.gpuEndTime - start))
            guard let self else { return }
            self.lock.lock()
            self.completions.append(completion)
            self.lock.unlock()
        }
    }

    /// Records what completed since the last frame, a device sample now and then, and saves
    /// the timeline when it is due.
    func flush(_ map: OpaquePointer, frame: UInt64, footprintMB: () -> Double) {
        lock.lock()
        let done = completions
        completions.removeAll()
        lock.unlock()
        for completion in done {
            span(map, frame: completion.frame, "copy-overlay", gpu: true, seconds: completion.gpu)
            maplibre_visionos_trace_presentation(
                map, completion.frame, FrameTraceRecorder.nanoseconds(completion.deadline),
                FrameTraceRecorder.nanoseconds(completion.completed))
        }
        frames += 1
        if frames % FrameTraceRecorder.deviceSampleEvery == 0 {
            let footprint = footprintMB()
            let bytes = footprint.isFinite ? UInt64(max(0, footprint) * 1_048_576) : 0
            maplibre_visionos_trace_device(
                map, frame, bytes, Int32(ProcessInfo.processInfo.thermalState.rawValue))
        }
        if CACurrentMediaTime() - lastExport >= FrameTraceRecorder.exportEverySeconds {
            lastExport = CACurrentMediaTime()
            export(map)
        }
    }

    private func export(_ map: OpaquePointer) {
        let required = maplibre_visionos_trace_export(map, nil, 0)
        guard required > 0, let exportURL else { return }
        var buffer = [CChar](repeating: 0, count: required)
        guard maplibre_visionos_trace_export(map, &buffer, required) == required else { return }
        var line = Data(buffer.prefix(required - 1).map { UInt8(bitPattern: $0) })
        line.append(0x0A)
        do {
            if !FileManager.default.fileExists(atPath: exportURL.path) {
                try Data().write(to: exportURL)
            }
            let file = try FileHandle(forWritingTo: exportURL)
            defer { try? file.close() }
            try file.seekToEnd()
            try file.write(contentsOf: line)
        } catch {
            maplibre_visionos_note("frame trace not saved: \(error)")
        }
    }

    private func span(_ map: OpaquePointer, frame: UInt64, _ name: String, gpu: Bool, seconds: Double) {
        name.withCString { maplibre_visionos_trace_span(map, frame, $0, gpu, FrameTraceRecorder.nanoseconds(seconds)) }
    }

    private static func nanoseconds(_ seconds: Double) -> UInt64 {
        UInt64(max(0, seconds) * 1e9)
    }
}
