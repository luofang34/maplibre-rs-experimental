import * as maplibre from "./wasm/maplibre"
import {initializeWasm} from "./wasm-instance.mjs";
import {checkRequirements, checkWasmFeatures, ensureWebGpuCompatibility} from "./browser";
import {preventDefaultTouchActions} from "./canvas";
import {MapLibreMap} from "./map";
import {computeMtTileWorkerCount, startOffscreenController} from "./offscreen-client";
// @ts-ignore esbuild plugin is handling this
import MultithreadedPoolWorker from './multithreaded/multithreaded-pool.worker.js';
// @ts-ignore esbuild plugin is handling this
import PoolWorker from './singlethreaded/pool.worker.js';
// @ts-ignore esbuild plugin is handling this
import RenderWorker from './singlethreaded/render.worker.js';

let mtWasmPrimed = false;
const prewarmedMtWorkers: Worker[] = [];

/**
 * Starts the map. `styleJson` is a MapLibre style document; without it the built-in style is used.
 *
 * `imageProviderModules` are URLs of JavaScript modules that make the images labels name, such
 * as road shields. The tile workers of a single-threaded build import each one and call its
 * default export with `{registerImageProvider(namespace, generation, draw)}`, where `draw`
 * is `(id, pixelRatio) => image | Promise<image>` and an image is `{width, height, data,
 * pixelRatio?, anchor?}`, `null` or `{status: "absent" | "unavailable" | "failed"}`. A
 * multithreaded build shares one registry with Rust providers instead.
 *
 * Resolves to the running map on the page's `maplibre` canvas, or to `undefined` when the
 * browser lacks what the map needs, which is reported to the user instead.
 */
export const startMapLibre = async (wasmPath: string | WebAssembly.Module | undefined, workerPath: string | undefined, styleJson?: string, imageProviderModules: string[] = [], canvasId?: string): Promise<MapLibreMap | undefined> => {
    await checkWasmFeatures()

    let message = checkRequirements();
    if (message) {
        console.error(message)
        alert(message)
        return
    }

    if (WEBGL) {
        const {Spector} = await import("spectorjs")
        let spector = new Spector()
        spector.displayUI()
    }

    if (!WEBGL) {
        ensureWebGpuCompatibility();
    }

    preventDefaultTouchActions();
    const targetCanvasId = ARENA_TELEMETRY && canvasId ? canvasId : "maplibre";
    const canvas = mapCanvas(targetCanvasId);

    // Default WebGPU path (single-threaded and multithreaded): run Map + WebGPU surface inside a
    // dedicated OffscreenCanvas Render Worker wired directly to Tile Workers over MessageChannel.
    if (!WEBGL && canvas && typeof canvas.transferControlToOffscreen === "function") {
        if (MULTITHREADED && imageProviderModules.length > 0) {
            console.warn("JavaScript image providers need a single-threaded build; register Rust providers through image_providers() instead.");
        }
        try {
            const controller = await startOffscreenController({
                canvas,
                wasmPath,
                workerPath,
                styleJson,
                imageProviderModules: MULTITHREADED ? [] : imageProviderModules,
                createRenderWorker: () => RenderWorker(),
                createPoolWorker: () => MULTITHREADED ? MultithreadedPoolWorker() : PoolWorker(),
            });
            return new MapLibreMap(controller, canvas);
        } catch (err) {
            // Fall back to main-thread winit only if canvas control was not already transferred.
            if (!(err instanceof DOMException && err.name === "InvalidStateError")) {
                throw err;
            }
            console.warn("OffscreenCanvas unavailable on canvas, falling back to main-thread surface:", err);
        }
    }

    const createMtWorker = (): Worker => workerPath ? new Worker(workerPath, {
        type: 'module',
    }) : MultithreadedPoolWorker();

    if (MULTITHREADED && !mtWasmPrimed) {
        mtWasmPrimed = true;
        const targetMtWorkers = computeMtTileWorkerCount();
        while (prewarmedMtWorkers.length < targetMtWorkers) {
            prewarmedMtWorkers.push(createMtWorker());
        }
    }

    await initializeWasm(wasmPath, undefined);

    const invokeRun = (workerFactory: any) => {
        if (ARENA_TELEMETRY && canvasId) {
            return (maplibre as any).run_maplibre_on_canvas(canvasId, workerFactory, styleJson);
        }
        return maplibre.run_maplibre(workerFactory, styleJson);
    };

    if (MULTITHREADED) {
        if (imageProviderModules.length > 0) {
            console.warn("JavaScript image providers need a single-threaded build; register Rust providers through image_providers() instead.")
        }
        const controller = await invokeRun(() => prewarmedMtWorkers.pop() ?? createMtWorker());
        return new MapLibreMap(controller, canvas)
    } else {
        const controller = await invokeRun((received_ptr: number) => {
            let worker: Worker = workerPath ? new Worker(workerPath, {
                type: 'module',
            }) : PoolWorker();  // Setting a "name" for this webworker is not yet supported, because it needs support from esbuild-plugin-inline-worker

            if (imageProviderModules.length > 0) {
                // Workers do not share the page's base URL, so they get absolute ones.
                worker.postMessage({
                    type: 'image_provider_modules',
                    urls: imageProviderModules.map(url => new URL(url, location.href).href),
                })
            }

            // Handle messages coming back from the Worker
            worker.onmessage = (message: MessageEvent<[tag: number, buffer: ArrayBuffer]>) => {
                // WARNING: Do not modify data passed from Rust!
                let data = message.data;

                const receive_data: (received_ptr: number, tag: number, buffer: ArrayBuffer) => void = maplibre["singlethreaded_receive_data"];

                if (!receive_data) {
                    throw Error("singlethreaded_main_entry is not defined. Maybe the Rust build used the wrong build configuration.")
                }

                receive_data(received_ptr, data[0], data[1])
            }

            return worker;
        });
        return new MapLibreMap(controller, canvas)
    }
}

/** The canvas `run_maplibre` draws on. */
const mapCanvas = (id = "maplibre"): HTMLCanvasElement => document.getElementById(id) as HTMLCanvasElement

export * from "./map"
