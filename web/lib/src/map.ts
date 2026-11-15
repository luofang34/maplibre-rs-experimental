import type {MapController} from "./wasm/maplibre"

/** A position as `[lng, lat]` or `{lng, lat}`, as GL JS `LngLatLike`. */
export type LngLatLike = [number, number] | { lng: number, lat: number }

/** A screen position in CSS pixels from the canvas's top left, as GL JS `PointLike`. */
export type PointLike = [number, number]

/** What a jump sets, as GL JS `CameraOptions`; angles are degrees and what is left out stays. */
export interface CameraOptions {
    center?: LngLatLike
    zoom?: number
    bearing?: number
    pitch?: number
    roll?: number
    elevation?: number
}

/** A camera move, as GL JS `EaseToOptions`; `duration` is in milliseconds. */
export interface EaseToOptions extends CameraOptions {
    duration?: number
}

/** A flight that zooms out to cross the distance, as GL JS `FlyToOptions`. */
export interface FlyToOptions extends EaseToOptions {
    curve?: number
    speed?: number
    minZoom?: number
}

/** Where the camera is. */
export interface CameraState {
    center: [number, number]
    zoom: number
    bearing: number
    pitch: number
}

/** The options of GL JS `queryRenderedFeatures`. */
export interface QueryRenderedFeaturesOptions {
    layers?: string[]
    filter?: unknown
}

/** A feature under the query, as GL JS `MapGeoJSONFeature` without its geometry. */
export interface MapFeature {
    type: "Feature"
    id?: number
    properties: Record<string, unknown>
    layer: { id: string }
    source: string | null
    sourceLayer: string
    geometryType: "Point" | "LineString" | "Polygon"
    text?: string
}

/** Feature-level changes to an inline GeoJSON source, as GL JS `GeoJSONSourceDiff`. */
export interface GeoJsonSourceDiff {
    removeAll?: boolean
    remove?: (string | number)[]
    add?: unknown[]
    update?: unknown[]
}

/** Per-frame telemetry emitted only when built with `--arena-telemetry`. */
export interface ArenaFrameTelemetry {
    frame: number
    hostFrame: number
    cpuMs: number
    gpuMs?: number | null
    uploadBytes: number
    draws: number
    passes?: number
    setVertexBuffers?: number
    writeBufferCalls?: number
    writeBufferBytes?: number
    drapeRedraws: number
    prefetchDrapes: number
    loaded: boolean
    settled: boolean
    wasmMemoryBytes: number
    gpuMemoryBytes: number
    pendingRequests: number
    totalRequests: number
    camera: CameraState
    redrawReasons: {
        changed: boolean
        loading: boolean
        awaitingImages: boolean
        cameraMoved: boolean
        workerResults: boolean
    }
    stages: Array<{ name: string; ms: number }>
}

interface ArenaControllerExt {
    setArenaCameraListener?: (callback: ((camera: CameraState) => void) | null) => void
    setArenaTelemetryListener?: (callback: ((telemetry: ArenaFrameTelemetry) => void) | null) => void
    waitArenaLoaded?: () => Promise<CameraState>
    waitArenaIdle?: () => Promise<CameraState>
}

/** A pointer event on the map. */
export interface MapMouseEvent {
    type: MapMouseEventType
    /** CSS pixels from the canvas's top left, which `queryRenderedFeatures` takes. */
    point: PointLike
    originalEvent: PointerEvent
}

export type MapMouseEventType = "click" | "mousemove"

type Listener = (event: MapMouseEvent) => void

/** How far, in CSS pixels, a press may move and still be a click, as GL JS `clickTolerance`. */
const CLICK_TOLERANCE = 3

/**
 * A running map. Methods carry GL JS's names and argument shapes; each queues a command the
 * map applies between frames, in call order, and returns a promise that settles once it has,
 * rejecting with the reason a command was refused.
 */
export class MapLibreMap {
    private readonly listeners = new Map<MapMouseEventType, Set<Listener>>()
    private pressed: { x: number, y: number, id: number } | null = null

    constructor(private readonly controller: MapController, readonly canvas: HTMLCanvasElement) {
        canvas.addEventListener("pointerdown", event => {
            if (event.isPrimary && event.button === 0) {
                this.pressed = {x: event.clientX, y: event.clientY, id: event.pointerId}
            }
        })
        canvas.addEventListener("pointerup", event => {
            const pressed = this.pressed
            this.pressed = null
            if (!pressed || pressed.id !== event.pointerId) return
            const moved = Math.hypot(event.clientX - pressed.x, event.clientY - pressed.y)
            if (moved <= CLICK_TOLERANCE) this.fire("click", event)
        })
        canvas.addEventListener("pointercancel", () => this.pressed = null)
        canvas.addEventListener("pointermove", event => {
            if (event.pointerType !== "touch") this.fire("mousemove", event)
        })
    }

    /** Calls `listener` for each `type` event, as GL JS `on`. */
    on(type: MapMouseEventType, listener: Listener): this {
        let listeners = this.listeners.get(type)
        if (!listeners) {
            listeners = new Set()
            this.listeners.set(type, listeners)
        }
        listeners.add(listener)
        return this
    }

    /** Stops calling `listener`, as GL JS `off`. */
    off(type: MapMouseEventType, listener: Listener): this {
        this.listeners.get(type)?.delete(listener)
        return this
    }

    private fire(type: MapMouseEventType, originalEvent: PointerEvent) {
        const listeners = this.listeners.get(type)
        if (!listeners || listeners.size === 0) return
        const rect = this.canvas.getBoundingClientRect()
        const point: PointLike = [originalEvent.clientX - rect.left, originalEvent.clientY - rect.top]
        for (const listener of [...listeners]) listener({type, point, originalEvent})
    }

    jumpTo(options: CameraOptions): Promise<void> {
        return this.controller.jumpTo(options)
    }

    /** Resolves to whether the camera arrived, rather than being stopped or moved. */
    easeTo(options: EaseToOptions): Promise<boolean> {
        return this.controller.easeTo(options)
    }

    /** Resolves to whether the camera arrived, rather than being stopped or moved. */
    flyTo(options: FlyToOptions): Promise<boolean> {
        return this.controller.flyTo(options)
    }

    stop(): Promise<void> {
        return this.controller.stop()
    }

    getCamera(): Promise<CameraState> {
        return this.controller.getCamera()
    }

    /** Replaces the active style with a style JSON string, URL, or style specification object, as GL JS `setStyle`. */
    setStyle(style: string | object): Promise<void> {
        return this.controller.setStyle(style)
    }

    setPaintProperty(layerId: string, name: string, value: unknown): Promise<void> {
        return this.controller.setPaintProperty(layerId, name, value)
    }

    setLayoutProperty(layerId: string, name: string, value: unknown): Promise<void> {
        return this.controller.setLayoutProperty(layerId, name, value)
    }

    setFilter(layerId: string, filter?: unknown): Promise<void> {
        return this.controller.setFilter(layerId, filter)
    }

    addLayer(layer: object, beforeId?: string): Promise<void> {
        return this.controller.addLayer(layer, beforeId)
    }

    removeLayer(id: string): Promise<void> {
        return this.controller.removeLayer(id)
    }

    setGlobalStateProperty(name: string, value: unknown): Promise<void> {
        return this.controller.setGlobalStateProperty(name, value)
    }

    getGlobalState(): Promise<Record<string, unknown>> {
        return this.controller.getGlobalState()
    }

    /** Replaces a GeoJSON source's data with a document or its URL, as GL JS `setData`. */
    setGeoJsonData(sourceId: string, data: object | string): Promise<void> {
        return this.controller.setGeoJsonData(sourceId, data)
    }

    /** Changes features of an inline GeoJSON source, as GL JS `updateData`. */
    updateGeoJsonData(sourceId: string, diff: GeoJsonSourceDiff): Promise<void> {
        return this.controller.updateGeoJsonData(sourceId, diff)
    }

    /**
     * Features drawn under a point or a box `[[x0, y0], [x1, y1]]`, or the whole canvas when
     * left out, topmost first. A single-threaded build finds symbols only, since its workers
     * send no index of fill and line geometry.
     */
    queryRenderedFeatures(geometry?: PointLike | [PointLike, PointLike], options?: QueryRenderedFeaturesOptions): Promise<MapFeature[]> {
        return this.controller.queryRenderedFeatures(geometry, options)
    }

    /** Subscribes to post-frame camera updates (no-op unless built with `--arena-telemetry`). */
    setArenaCameraListener(listener: ((camera: CameraState) => void) | null): void {
        if (ARENA_TELEMETRY) {
            (this.controller as unknown as ArenaControllerExt).setArenaCameraListener?.(listener)
        }
    }

    /** Subscribes to per-frame stats and stage breakdown (no-op unless built with `--arena-telemetry`). */
    setArenaTelemetryListener(listener: ((telemetry: ArenaFrameTelemetry) => void) | null): void {
        if (ARENA_TELEMETRY) {
            (this.controller as unknown as ArenaControllerExt).setArenaTelemetryListener?.(listener)
        }
    }

    /** Resolves once initial tile/glyph/sprite loading has finished (without waiting for fade animations). */
    waitArenaLoaded(): Promise<CameraState> {
        if (ARENA_TELEMETRY) {
            const fn = (this.controller as unknown as ArenaControllerExt).waitArenaLoaded
            if (fn) return fn.call(this.controller)
        }
        return this.waitArenaIdle()
    }

    /** Resolves once the map has settled after loading/animating (falls back to `getCamera` without `--arena-telemetry`). */
    waitArenaIdle(): Promise<CameraState> {
        if (ARENA_TELEMETRY) {
            const fn = (this.controller as unknown as ArenaControllerExt).waitArenaIdle
            if (fn) return fn.call(this.controller)
        }
        return this.getCamera()
    }

    /** Tears down the running map event loop, workers, and GPU resources, as GL JS `remove`. */
    remove(): void {
        this.listeners.clear()
        this.controller.remove()
    }

    /** Alias for `remove()`. */
    destroy(): void {
        this.remove()
    }
}
