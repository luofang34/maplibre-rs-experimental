import * as maplibre from "./wasm/maplibre"
import {initializeWasm} from "./wasm-instance.mjs";
import {
    checkRequirements,
    ensureWebGpuCompatibility,
    isWebGLSupported,
    selectGraphicsBackend,
} from "./browser";
import {preventDefaultTouchActions} from "./canvas";
import {MapLibreMap} from "./map";
import {
    compileWasmModule,
    computeMtEagerWorkerCount,
    computeStTileWorkerCount,
    startOffscreenController,
    type WasmModuleSource,
} from "./offscreen-client";
import {normalizeStyleInput} from "./style-input.mjs";
// @ts-ignore esbuild plugin is handling this
import MultithreadedPoolWorker from './multithreaded/multithreaded-pool.worker.js';
// @ts-ignore esbuild plugin is handling this
import PoolWorker from './singlethreaded/pool.worker.js';
// @ts-ignore esbuild plugin is handling this
import RenderWorker from './singlethreaded/render.worker.js';

export type StyleInput =
    | string
    | Record<string, unknown>
    | Promise<string | Record<string, unknown> | undefined>
    | undefined;

export type TransformRequestResult =
    | string
    | { url: string; headers?: HeadersInit; credentials?: RequestCredentials; mode?: RequestMode }
    | null
    | undefined;

export type TransformRequestFn = (url: string, resourceType: string) => TransformRequestResult;

export interface StartMapOptions {
    /** Optional URL, `WebAssembly.Module`, or pending promise for `maplibre_tile_worker_bg.wasm`. */
    workerWasmPath?: WasmModuleSource;
    /** Optional fallback WebGL2 `module.js` URL when WebGPU is unavailable at runtime. */
    webglModulePath?: string;
    /** Optional fallback WebGL2 `.wasm` source paired with `webglModulePath`. */
    webglWasmPath?: WasmModuleSource;
    /** Optional request rewriter applied when `styleJson` is passed as a style URL string. */
    transformRequest?: TransformRequestFn;
}

let mtWasmPrimed = false;
const prewarmedMtWorkers: Worker[] = [];

const loadWebGlFallback = async (
    workerPath: string | undefined,
    styleJson: string | Promise<string | undefined> | undefined,
    imageProviderModules: string[],
    canvasId: string | undefined,
    options: StartMapOptions | undefined,
): Promise<MapLibreMap | undefined> => {
    const relFallback = ["webgl", "module.js"].join("/");
    const webglUrl = options?.webglModulePath ?? new URL(relFallback, import.meta.url).href;
    try {
        const [resolvedStyle, webglMod] = await Promise.all([
            Promise.resolve(styleJson),
            import(/* @vite-ignore */ webglUrl),
        ]);
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
};

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
    styleJson?: StyleInput,
    imageProviderModules: string[] = [],
    canvasId?: string,
    options?: StartMapOptions,
): Promise<MapLibreMap | undefined> => {
    const secureCtx = typeof isSecureContext === "boolean" ? isSecureContext : false;
    const coi = typeof crossOriginIsolated === "boolean" ? crossOriginIsolated : false;
    const hasWebGpuFeature = Boolean(
        typeof navigator !== "undefined" &&
        "gpu" in navigator &&
        typeof (navigator as {gpu?: {requestAdapter?: unknown}}).gpu?.requestAdapter === "function",
    );

    if (WEBGL) {
        const message = checkRequirements();
        if (message) {
            console.error(message);
            alert(message);
            return;
        }
    } else {
        const mtEnvReady = !MULTITHREADED || (secureCtx && coi);
        const earlyDecision = selectGraphicsBackend({
            webgl: false,
            hasWebGpu: hasWebGpuFeature,
            hasWebGl: mtEnvReady && hasWebGpuFeature ? false : isWebGLSupported(),
            hasWebglFallbackModule: true,
            multithreaded: MULTITHREADED,
            crossOriginIsolated: coi,
            isSecureContext: secureCtx,
        });
        if (earlyDecision.backend === "fallback-webgl2") {
            if (MULTITHREADED && !mtEnvReady) {
                console.warn(
                    "Multithreaded WebGPU requires crossOriginIsolated (COOP/COEP) in a secure context; falling back to single-threaded WebGL2.",
                );
            }
            const normalizedFallbackStyle = normalizeStyleInput(
                styleJson,
                options?.transformRequest,
            );
            const fallbackStyle: string | Promise<string | undefined> | undefined =
                normalizedFallbackStyle.resolved
                    ? (normalizedFallbackStyle.value as string | undefined)
                    : (normalizedFallbackStyle.promise as Promise<string | undefined>);
            return loadWebGlFallback(
                workerPath,
                fallbackStyle,
                imageProviderModules,
                canvasId,
                options,
            );
        }
        if (earlyDecision.backend === "error") {
            console.error(earlyDecision.message);
            alert(earlyDecision.message);
            return undefined;
        }
    }

    const styleAbort = typeof AbortController === "function" ? new AbortController() : undefined;
    const normalizedStyle = normalizeStyleInput(styleJson, options?.transformRequest, {
        signal: styleAbort?.signal,
    });
    const styleSource: string | Promise<string | undefined> | undefined = normalizedStyle.resolved
        ? (normalizedStyle.value as string | undefined)
        : (normalizedStyle.promise as Promise<string | undefined>);
    if (!normalizedStyle.resolved) {
        (styleSource as Promise<string | undefined>).catch(() => {});
    }

    const mainModulePrefetch = compileWasmModule(wasmPath);
    const workerModulePrefetch =
        !MULTITHREADED
            ? compileWasmModule(
                  options?.workerWasmPath ??
                      new URL("./wasm/maplibre_tile_worker_bg.wasm", import.meta.url).href,
              )
            : undefined;
    const preRenderWorker =
        !workerPath &&
        typeof Worker !== "undefined" &&
        (WEBGL ? isWebGLSupported() : hasWebGpuFeature)
            ? RenderWorker()
            : undefined;
    const prePoolWorkers: Worker[] = preRenderWorker
        ? Array.from(
              {length: MULTITHREADED ? computeMtEagerWorkerCount() : computeStTileWorkerCount()},
              () => MULTITHREADED ? MultithreadedPoolWorker() : PoolWorker(),
          )
        : [];
    const terminatePrewarmedWorkers = () => {
        preRenderWorker?.terminate();
        while (prePoolWorkers.length > 0) {
            prePoolWorkers.pop()?.terminate();
        }
    };

    if (!WEBGL) {
        ensureWebGpuCompatibility(globalThis, {
            sharedMemory: MULTITHREADED,
            traceGpuCalls: ARENA_TELEMETRY,
        });
    }

    preventDefaultTouchActions();
    const targetCanvasId = ARENA_TELEMETRY && canvasId ? canvasId : "maplibre";
    const canvas = mapCanvas(targetCanvasId);

    // Default path (WebGPU & WebGL2, single-threaded & multithreaded): run Map + GPU surface
    // inside a dedicated OffscreenCanvas Render Worker wired directly to Tile Workers over MessageChannel.
    if (canvas && typeof canvas.transferControlToOffscreen === "function") {
        if (MULTITHREADED && imageProviderModules.length > 0) {
            console.warn("JavaScript image providers need a single-threaded build; register Rust providers through image_providers() instead.");
        }
        try {
            const controller = await startOffscreenController({
                canvas,
                wasmPath: mainModulePrefetch ?? wasmPath,
                workerWasmPath: workerModulePrefetch ?? options?.workerWasmPath,
                workerPath,
                styleJson: styleSource,
                styleAbort,
                imageProviderModules: MULTITHREADED ? [] : imageProviderModules,
                createRenderWorker: () => RenderWorker(),
                createPoolWorker: () => MULTITHREADED ? MultithreadedPoolWorker() : PoolWorker(),
                prewarmedRenderWorker: preRenderWorker,
                prewarmedPoolWorkers: prePoolWorkers,
            });
            return new MapLibreMap(controller, canvas);
        } catch (err) {
            terminatePrewarmedWorkers();
            // Fall back to WebGL2 if the WebGPU worker failed at runtime (e.g. null GPUAdapter),
            // cloning the canvas in case `transferControlToOffscreen()` already detached it.
            if (!WEBGL && !(err instanceof DOMException && err.name === "InvalidStateError") && isWebGLSupported()) {
                const freshCanvas = canvas.cloneNode(false) as HTMLCanvasElement;
                canvas.replaceWith(freshCanvas);
                return loadWebGlFallback(
                    workerPath,
                    styleSource,
                    imageProviderModules,
                    canvasId,
                    options,
                );
            }
            styleAbort?.abort();
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
        const targetMtWorkers = computeMtEagerWorkerCount();
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
        Promise.resolve(styleSource),
        initializeWasm(resolvedWasm, undefined),
    ]);

    const invokeRun = (workerFactory: any) => {
        if (ARENA_TELEMETRY && canvasId && typeof (maplibre as any).run_maplibre_on_canvas === "function") {
            return (maplibre as any).run_maplibre_on_canvas(canvasId, workerFactory, resolvedStyleJson);
        }
        if (typeof (maplibre as any).run_maplibre !== "function") {
            throw new Error("OffscreenCanvas is required for this build.");
        }
        return (maplibre as any).run_maplibre(workerFactory, resolvedStyleJson);
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
