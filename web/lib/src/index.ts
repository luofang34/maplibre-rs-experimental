import * as maplibre from "./wasm/maplibre"
import {initializeWasm} from "./wasm-instance.mjs";
import {
    checkRequirements,
    checkWasmFeatures,
    ensureWebGpuCompatibility,
    isWebGLSupported,
    probeWebGpuAvailable,
    selectGraphicsBackend,
} from "./browser";
import {preventDefaultTouchActions} from "./canvas";
import {MapLibreMap} from "./map";
import {
    compileWasmModule,
    computeMtTileWorkerCount,
    ST_TILE_WORKER_COUNT,
    startOffscreenController,
    type WasmModuleSource,
} from "./offscreen-client";
// @ts-ignore esbuild plugin is handling this
import MultithreadedPoolWorker from './multithreaded/multithreaded-pool.worker.js';
// @ts-ignore esbuild plugin is handling this
import PoolWorker from './singlethreaded/pool.worker.js';
// @ts-ignore esbuild plugin is handling this
import RenderWorker from './singlethreaded/render.worker.js';

export interface StartMapOptions {
    /** Optional URL, `WebAssembly.Module`, or pending promise for `maplibre_tile_worker_bg.wasm`. */
    workerWasmPath?: WasmModuleSource;
    /** Optional fallback WebGL2 `module.js` URL when WebGPU is unavailable at runtime. */
    webglModulePath?: string;
    /** Optional fallback WebGL2 `.wasm` source paired with `webglModulePath`. */
    webglWasmPath?: WasmModuleSource;
}

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
export const startMapLibre = async (
    wasmPath: WasmModuleSource,
    workerPath: string | undefined,
    styleJson?: string | Promise<string | undefined>,
    imageProviderModules: string[] = [],
    canvasId?: string,
    options?: StartMapOptions,
): Promise<MapLibreMap | undefined> => {
    let message = checkRequirements();
    if (message) {
        console.error(message)
        alert(message)
        return
    }

    const mainModulePrefetch = !WEBGL ? compileWasmModule(wasmPath) : undefined;
    const workerModulePrefetch =
        !WEBGL && !MULTITHREADED
            ? compileWasmModule(
                  options?.workerWasmPath ??
                      new URL("./wasm/maplibre_tile_worker_bg.wasm", import.meta.url).href,
              )
            : undefined;
    const preRenderWorker =
        !WEBGL && !workerPath && typeof navigator !== "undefined" && "gpu" in navigator
            ? RenderWorker()
            : undefined;
    const prePoolWorkers: Worker[] = preRenderWorker
        ? Array.from(
              {length: MULTITHREADED ? computeMtTileWorkerCount() : ST_TILE_WORKER_COUNT},
              () => MULTITHREADED ? MultithreadedPoolWorker() : PoolWorker(),
          )
        : [];
    const terminatePrewarmedWorkers = () => {
        preRenderWorker?.terminate();
        while (prePoolWorkers.length > 0) {
            prePoolWorkers.pop()?.terminate();
        }
    };

    if (WEBGL) {
        await checkWasmFeatures();
        const {Spector} = await import("spectorjs")
        let spector = new Spector()
        spector.displayUI()
    } else {
        void checkWasmFeatures();
        const hasWebGpu = await probeWebGpuAvailable();
        const decision = selectGraphicsBackend({
            webgl: false,
            hasWebGpu,
            hasWebGl: isWebGLSupported(),
            hasWebglFallbackModule: true,
        });
        if (decision.backend === "fallback-webgl2") {
            terminatePrewarmedWorkers();
            const relFallback = ["webgl", "module.js"].join("/");
            const webglUrl = options?.webglModulePath ?? new URL(relFallback, import.meta.url).href;
            const resolvedStyle = await styleJson;
            try {
                const webglMod = await import(/* @vite-ignore */ webglUrl);
                if (typeof webglMod?.startMapLibre === "function") {
                    console.warn("WebGPU unavailable in this browser; falling back automatically to WebGL2.");
                    return webglMod.startMapLibre(
                        options?.webglWasmPath,
                        workerPath,
                        resolvedStyle,
                        imageProviderModules,
                        canvasId,
                        {workerWasmPath: options?.workerWasmPath},
                    );
                }
            } catch (err) {
                console.warn("WebGL2 fallback module failed to load:", err);
            }
            console.error("WebGPU is unavailable and WebGL2 fallback could not be loaded.");
            return undefined;
        }
        if (decision.backend === "error") {
            terminatePrewarmedWorkers();
            console.error(decision.message);
            alert(decision.message);
            return undefined;
        }
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
                wasmPath: mainModulePrefetch ?? wasmPath,
                workerWasmPath: workerModulePrefetch ?? options?.workerWasmPath,
                workerPath,
                styleJson,
                imageProviderModules: MULTITHREADED ? [] : imageProviderModules,
                createRenderWorker: () => RenderWorker(),
                createPoolWorker: () => MULTITHREADED ? MultithreadedPoolWorker() : PoolWorker(),
                prewarmedRenderWorker: preRenderWorker,
                prewarmedPoolWorkers: prePoolWorkers,
            });
            return new MapLibreMap(controller, canvas);
        } catch (err) {
            terminatePrewarmedWorkers();
            // Fall back to main-thread winit only if canvas control was not already transferred.
            if (!(err instanceof DOMException && err.name === "InvalidStateError")) {
                throw err;
            }
            console.warn("OffscreenCanvas unavailable on canvas, falling back to main-thread surface:", err);
        }
    } else {
        terminatePrewarmedWorkers();
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

    const workerModulePromise = (!MULTITHREADED)
        ? (workerModulePrefetch ?? compileWasmModule(
            options?.workerWasmPath ?? new URL("./wasm/maplibre_tile_worker_bg.wasm", import.meta.url).href,
        ))
        : null;
    const wasmSource = mainModulePrefetch ?? wasmPath;
    const resolvedWasm = wasmSource && typeof (wasmSource as Promise<WebAssembly.Module>).then === "function"
        ? await wasmSource
        : (wasmSource as string | WebAssembly.Module | undefined);
    const [workerModule, resolvedStyleJson] = await Promise.all([
        workerModulePromise,
        Promise.resolve(styleJson),
        initializeWasm(resolvedWasm, undefined),
    ]);

    const invokeRun = (workerFactory: any) => {
        if (ARENA_TELEMETRY && canvasId) {
            return (maplibre as any).run_maplibre_on_canvas(canvasId, workerFactory, resolvedStyleJson);
        }
        return maplibre.run_maplibre(workerFactory, resolvedStyleJson);
    };

    if (MULTITHREADED) {
        if (imageProviderModules.length > 0) {
            console.warn("JavaScript image providers need a single-threaded build; register Rust providers through image_providers() instead.")
        }
        const controller = await invokeRun(() => prewarmedMtWorkers.pop() ?? createMtWorker());
        return new MapLibreMap(controller, canvas)
    } else {
        const spawnedWorkers: Worker[] = [];
        const controller = await invokeRun(() => {
            const worker: Worker = workerPath ? new Worker(workerPath, {
                type: 'module',
            }) : PoolWorker();
            spawnedWorkers.push(worker);

            if (imageProviderModules.length > 0) {
                // Workers do not share the page's base URL, so they get absolute ones.
                worker.postMessage({
                    type: 'image_provider_modules',
                    urls: imageProviderModules.map(url => new URL(url, location.href).href),
                });
            }

            const channel = new MessageChannel();
            worker.postMessage({type: 'wasm_init', module: workerModule, port: channel.port2}, [channel.port2]);
            return channel.port1;
        });
        const terminateWorkers = () => {
            for (const worker of spawnedWorkers.splice(0)) {
                worker.terminate();
            }
        };
        const origDestroy = controller.destroy?.bind(controller);
        if (origDestroy) {
            controller.destroy = () => {
                try {
                    return origDestroy();
                } finally {
                    terminateWorkers();
                }
            };
        }
        const origRemove = controller.remove?.bind(controller);
        if (origRemove) {
            controller.remove = () => {
                try {
                    return origRemove();
                } finally {
                    terminateWorkers();
                }
            };
        }
        return new MapLibreMap(controller, canvas)
    }
}

/** The canvas `run_maplibre` draws on. */
const mapCanvas = (id = "maplibre"): HTMLCanvasElement => document.getElementById(id) as HTMLCanvasElement

export * from "./map"
