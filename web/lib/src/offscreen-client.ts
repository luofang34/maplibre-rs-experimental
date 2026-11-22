import type {MapController} from "./wasm/maplibre";
import type {ArenaFrameTelemetry, CameraState} from "./map";
import {createOffscreenCoalescer} from "./offscreen-coalesce.mjs";

type Waiter = { resolve: (v: any) => void, reject: (e: any) => void };

export type WasmModuleSource = string | WebAssembly.Module | Promise<WebAssembly.Module> | undefined;

const compiledModuleCache = new Map<string, Promise<WebAssembly.Module>>();

export const compileWasmModule = (
    wasmPath: WasmModuleSource,
): Promise<WebAssembly.Module> => {
    if (wasmPath instanceof WebAssembly.Module) {
        return Promise.resolve(wasmPath);
    }
    if (wasmPath && typeof (wasmPath as Promise<WebAssembly.Module>).then === "function") {
        return wasmPath as Promise<WebAssembly.Module>;
    }
    const url = (wasmPath as string | undefined) ?? new URL("./wasm/maplibre_bg.wasm", import.meta.url).href;
    const cached = compiledModuleCache.get(url);
    if (cached) {
        return cached;
    }
    const promise = (async () => {
        const responsePromise = fetch(url);
        if (typeof WebAssembly.compileStreaming === "function") {
            try {
                return await WebAssembly.compileStreaming(responsePromise);
            } catch {
                const fallback = await fetch(url);
                return WebAssembly.compile(await fallback.arrayBuffer());
            }
        }
        const resolved = await responsePromise;
        return WebAssembly.compile(await resolved.arrayBuffer());
    })().catch(err => {
        compiledModuleCache.delete(url);
        throw err;
    });
    compiledModuleCache.set(url, promise);
    return promise;
};

interface OffscreenStartArgs {
    canvas: HTMLCanvasElement
    wasmPath: WasmModuleSource
    workerWasmPath?: WasmModuleSource
    workerPath: string | undefined
    styleJson?: string | Promise<string | undefined>
    imageProviderModules: string[]
    createRenderWorker: () => Worker
    createPoolWorker: () => Worker
    prewarmedRenderWorker?: Worker
    prewarmedPoolWorkers?: Worker[]
}

export const ST_TILE_WORKER_COUNT = 4;

export const computeStTileWorkerCount = (): number => {
    const cores = Number(globalThis.navigator?.hardwareConcurrency) || 4;
    return Math.min(6, Math.max(ST_TILE_WORKER_COUNT, Math.floor(cores / 2)));
};

export const computeMtTileWorkerCount = (): number => {
    const cores = Number(globalThis.navigator?.hardwareConcurrency) || 4;
    return Math.min(4, Math.max(3, Math.floor(cores / 2)));
};

const createSharedWasmMemory = (): WebAssembly.Memory | undefined => {
    if (
        !MULTITHREADED
        || typeof WebAssembly.Memory !== "function"
        || typeof (globalThis as {SharedArrayBuffer?: unknown}).SharedArrayBuffer !== "function"
    ) {
        return undefined;
    }
    try {
        const initial = Math.min(MT_MAX_MEMORY_PAGES, Math.max(MT_INITIAL_MEMORY_PAGES, 64));
        return new WebAssembly.Memory({initial, maximum: MT_MAX_MEMORY_PAGES, shared: true});
    } catch {
        return undefined;
    }
};

export const startOffscreenController = async ({
    canvas,
    wasmPath,
    workerWasmPath,
    workerPath,
    styleJson,
    imageProviderModules,
    createRenderWorker,
    createPoolWorker,
    prewarmedRenderWorker,
    prewarmedPoolWorkers,
}: OffscreenStartArgs): Promise<MapController> => {
    const offscreen = canvas.transferControlToOffscreen();

    // Spawn render worker + tile workers immediately at t=0 in parallel with WASM compilation.
    const workerCount = MULTITHREADED ? computeMtTileWorkerCount() : computeStTileWorkerCount();
    const renderWorker = prewarmedRenderWorker ?? createRenderWorker();
    const tileWorkers: Worker[] = [];
    const channels: MessageChannel[] = [];
    const providerUrls = imageProviderModules.map(url => new URL(url, location.href).href);

    for (let i = 0; i < workerCount; i++) {
        const worker = !workerPath && prewarmedPoolWorkers?.length
            ? prewarmedPoolWorkers.pop()!
            : (workerPath ? new Worker(workerPath, {type: "module"}) : createPoolWorker());
        if (!MULTITHREADED && providerUrls.length > 0) {
            worker.postMessage({type: "image_provider_modules", urls: providerUrls});
        }
        const channel = new MessageChannel();
        if (MULTITHREADED) {
            const port2 = channel.port2;
            worker.postMessage({type: "bind_port", port: port2}, [port2]);
        }
        tileWorkers.push(worker);
        channels.push(channel);
    }

    let nextRpcId = 1;
    let closed = false;
    const pending = new Map<number, Waiter>();
    let cameraListener: ((camera: CameraState) => void) | null = null;
    let telemetryListener: ((telemetry: ArenaFrameTelemetry) => void) | null = null;

    const rawRpc = (method: string, args: unknown[], transfer?: Transferable[]): Promise<any> => {
        if (closed) return Promise.resolve(undefined);
        const id = nextRpcId++;
        return new Promise((resolve, reject) => {
            pending.set(id, {resolve, reject});
            if (transfer && transfer.length > 0) {
                renderWorker.postMessage({type: "rpc", id, method, args}, transfer);
            } else {
                renderWorker.postMessage({type: "rpc", id, method, args});
            }
        });
    };

    const coalescer = createOffscreenCoalescer(
        msg => renderWorker.postMessage(msg),
        rawRpc,
        () => closed,
    );

    const rejectAll = (reason: unknown) => {
        for (const entry of pending.values()) {
            entry.reject(reason);
        }
        pending.clear();
        coalescer.rejectQueued(reason);
    };

    const terminateAllWorkers = () => {
        renderWorker.terminate();
        for (const w of tileWorkers) {
            w.terminate();
        }
    };

    renderWorker.onmessage = (event: MessageEvent<any>) => {
        const msg = event.data;
        if (!msg) return;
        if (msg.type === "rpc_ok") {
            const waiter = pending.get(msg.id);
            if (waiter) {
                pending.delete(msg.id);
                waiter.resolve(msg.result);
            }
        } else if (msg.type === "rpc_err") {
            const waiter = pending.get(msg.id);
            if (waiter) {
                pending.delete(msg.id);
                waiter.reject(new Error(msg.error));
            }
        } else if (msg.type === "camera") {
            cameraListener?.(msg.payload);
        } else if (msg.type === "telemetry") {
            telemetryListener?.(msg.payload);
        } else if (msg.type === "destroyed") {
            terminateAllWorkers();
        }
    };

    renderWorker.onerror = err => {
        rejectAll(err);
        if (closed) {
            terminateAllWorkers();
        }
    };

    const callRpc = (method: string, args: unknown[]): Promise<any> => {
        coalescer.flushBeforeOrderedRpc();
        return rawRpc(method, args);
    };

    const cleanupFns: Array<() => void> = [];
    const teardown = () => {
        if (closed) return;
        closed = true;
        cameraListener = null;
        telemetryListener = null;
        for (const fn of cleanupFns.splice(0)) {
            fn();
        }
        let destroyDispatched = false;
        try {
            renderWorker.postMessage({type: "destroy"});
            destroyDispatched = true;
        } catch {
            // Worker is unreachable or already terminated.
        }
        rejectAll(new Error("the map's host has stopped"));
        if (!destroyDispatched) {
            terminateAllWorkers();
        }
    };

    try {
        const mainModulePromise = compileWasmModule(wasmPath);
        const sharedMemory = createSharedWasmMemory();
        let workerReadyPromise: Promise<void> = Promise.resolve();
        if (!MULTITHREADED) {
            const resolvedWorkerWasm =
                workerWasmPath ?? new URL("./wasm/maplibre_tile_worker_bg.wasm", import.meta.url).href;
            workerReadyPromise = compileWasmModule(resolvedWorkerWasm).then(workerModule => {
                if (closed) return;
                for (let i = 0; i < workerCount; i++) {
                    const port2 = channels[i].port2;
                    tileWorkers[i].postMessage(
                        {type: "wasm_init", module: workerModule, port: port2},
                        [port2],
                    );
                }
            });
        }

        const measure = () => {
            const dpr = window.devicePixelRatio || 1;
            const rect = canvas.getBoundingClientRect();
            const cssW = rect.width || canvas.clientWidth || 800;
            const cssH = rect.height || canvas.clientHeight || 600;
            return {
                width: Math.max(1, Math.round(cssW * dpr)),
                height: Math.max(1, Math.round(cssH * dpr)),
                scaleFactor: dpr,
            };
        };

        const initial = measure();
        const isThenable =
            styleJson != null && typeof (styleJson as Promise<string | undefined>).then === "function";
        let styleResolved = !isThenable;
        let resolvedStyleValue: string | undefined = isThenable ? undefined : (styleJson as string | undefined);
        const stylePromise = isThenable
            ? Promise.resolve(styleJson).then(value => {
                  resolvedStyleValue = value;
                  styleResolved = true;
                  return value;
              })
            : Promise.resolve(resolvedStyleValue);

        const renderInitPromise = mainModulePromise.then(module => {
            if (closed) return;
            if (MULTITHREADED && sharedMemory) {
                for (const worker of tileWorkers) {
                    worker.postMessage({type: "wasm_init", module, memory: sharedMemory});
                }
            }
            const ports = channels.map(ch => ch.port1);
            if (styleResolved) {
                const initId = nextRpcId++;
                return new Promise<void>((resolve, reject) => {
                    pending.set(initId, {resolve, reject});
                    renderWorker.postMessage(
                        {
                            type: "init",
                            id: initId,
                            module,
                            memory: sharedMemory,
                            canvas: offscreen,
                            width: initial.width,
                            height: initial.height,
                            scaleFactor: initial.scaleFactor,
                            ports,
                            styleJson: resolvedStyleValue,
                        },
                        [offscreen, ...ports],
                    );
                });
            }
            renderWorker.postMessage(
                {
                    type: "preinit",
                    module,
                    memory: sharedMemory,
                    canvas: offscreen,
                    width: initial.width,
                    height: initial.height,
                    scaleFactor: initial.scaleFactor,
                    ports,
                },
                [offscreen, ...ports],
            );
            return stylePromise.then(resolvedStyleJson => {
                if (closed) return;
                const initId = nextRpcId++;
                return new Promise<void>((resolve, reject) => {
                    pending.set(initId, {resolve, reject});
                    renderWorker.postMessage({
                        type: "finish_init",
                        id: initId,
                        styleJson: resolvedStyleJson,
                    });
                });
            });
        });
        await Promise.all([renderInitPromise, workerReadyPromise]);

        // Attach DOM resize + DPR + input forwarding listeners.
        if (canvas.tabIndex < 0) {
            canvas.tabIndex = 0;
        }
        let lastW = initial.width;
        let lastH = initial.height;
        let lastDpr = initial.scaleFactor;
        const syncSize = () => {
            if (closed) return;
            const cur = measure();
            if (cur.width !== lastW || cur.height !== lastH || cur.scaleFactor !== lastDpr) {
                coalescer.flushInput();
                lastW = cur.width;
                lastH = cur.height;
                lastDpr = cur.scaleFactor;
                renderWorker.postMessage({type: "resize", ...cur});
            }
        };

        if (typeof ResizeObserver !== "undefined") {
            const ro = new ResizeObserver(syncSize);
            ro.observe(canvas);
            cleanupFns.push(() => ro.disconnect());
        }
        window.addEventListener("resize", syncSize);
        cleanupFns.push(() => window.removeEventListener("resize", syncSize));

        const localPoint = (clientX: number, clientY: number): [number, number] => {
            const rect = canvas.getBoundingClientRect();
            return [clientX - rect.left, clientY - rect.top];
        };

        const onPointerDown = (e: PointerEvent) => {
            if (e.pointerType === "touch") return;
            coalescer.flushInput();
            canvas.focus({preventScroll: true});
            try {
                canvas.setPointerCapture(e.pointerId);
            } catch {
                // Synthetic events may not have an active pointerId.
            }
            const [x, y] = localPoint(e.clientX, e.clientY);
            renderWorker.postMessage({
                type: "mods",
                shift: e.shiftKey,
                alt: e.altKey,
                ctrl: e.ctrlKey || e.metaKey,
            });
            renderWorker.postMessage({type: "pbtn", button: e.button, pressed: true, x, y});
        };
        const onPointerMove = (e: PointerEvent) => {
            if (e.pointerType === "touch") return;
            const [x, y] = localPoint(e.clientX, e.clientY);
            coalescer.queuePointerMove(x, y);
        };
        const onPointerUp = (e: PointerEvent) => {
            if (e.pointerType === "touch") return;
            coalescer.flushInput();
            const [x, y] = localPoint(e.clientX, e.clientY);
            renderWorker.postMessage({type: "pbtn", button: e.button, pressed: false, x, y});
        };
        const onWheel = (e: WheelEvent) => {
            e.preventDefault();
            const [x, y] = localPoint(e.clientX, e.clientY);
            coalescer.queueWheel({
                x,
                y,
                dx: e.deltaX,
                dy: e.deltaY,
                mode: e.deltaMode,
                ctrl: e.ctrlKey || e.metaKey,
                shift: e.shiftKey,
                alt: e.altKey,
            });
        };
        const forwardTouches = (phase: number) => (e: TouchEvent) => {
            if (phase !== 1) {
                coalescer.flushInput();
            }
            const rect = canvas.getBoundingClientRect();
            for (let i = 0; i < e.changedTouches.length; i++) {
                const t = e.changedTouches[i];
                const id = t.identifier >>> 0;
                const x = t.clientX - rect.left;
                const y = t.clientY - rect.top;
                if (phase === 1) {
                    coalescer.queueTouchMove(id, x, y);
                } else {
                    renderWorker.postMessage({type: "touch", id, phase, x, y});
                }
            }
        };
        const onTouchStart = forwardTouches(0);
        const onTouchMove = forwardTouches(1);
        const onTouchEnd = forwardTouches(2);
        const onTouchCancel = forwardTouches(3);
        const onKeyDown = (e: KeyboardEvent) => {
            coalescer.flushInput();
            renderWorker.postMessage({
                type: "mods",
                shift: e.shiftKey,
                alt: e.altKey,
                ctrl: e.ctrlKey || e.metaKey,
            });
            renderWorker.postMessage({type: "key", key: e.key, pressed: true});
        };
        const onKeyUp = (e: KeyboardEvent) => {
            coalescer.flushInput();
            renderWorker.postMessage({
                type: "mods",
                shift: e.shiftKey,
                alt: e.altKey,
                ctrl: e.ctrlKey || e.metaKey,
            });
            renderWorker.postMessage({type: "key", key: e.key, pressed: false});
        };

        canvas.addEventListener("pointerdown", onPointerDown);
        canvas.addEventListener("pointermove", onPointerMove);
        canvas.addEventListener("pointerup", onPointerUp);
        canvas.addEventListener("pointercancel", onPointerUp);
        canvas.addEventListener("wheel", onWheel, {passive: false});
        canvas.addEventListener("touchstart", onTouchStart, {passive: false});
        canvas.addEventListener("touchmove", onTouchMove, {passive: false});
        canvas.addEventListener("touchend", onTouchEnd);
        canvas.addEventListener("touchcancel", onTouchCancel);
        canvas.addEventListener("keydown", onKeyDown);
        canvas.addEventListener("keyup", onKeyUp);

        cleanupFns.push(() => {
            canvas.removeEventListener("pointerdown", onPointerDown);
            canvas.removeEventListener("pointermove", onPointerMove);
            canvas.removeEventListener("pointerup", onPointerUp);
            canvas.removeEventListener("pointercancel", onPointerUp);
            canvas.removeEventListener("wheel", onWheel);
            canvas.removeEventListener("touchstart", onTouchStart);
            canvas.removeEventListener("touchmove", onTouchMove);
            canvas.removeEventListener("touchend", onTouchEnd);
            canvas.removeEventListener("touchcancel", onTouchCancel);
            canvas.removeEventListener("keydown", onKeyDown);
            canvas.removeEventListener("keyup", onKeyUp);
        });
    } catch (err) {
        teardown();
        throw err;
    }

    const proxy: Record<string, any> = {
        jumpTo: (options: unknown) => coalescer.jumpTo(options),
        easeTo: (options: unknown) => callRpc("easeTo", [options]),
        flyTo: (options: unknown) => callRpc("flyTo", [options]),
        stop: () => callRpc("stop", []),
        getCamera: () => callRpc("getCamera", []),
        setStyle: (style: unknown) => callRpc("setStyle", [style]),
        setPaintProperty: (layer: string, name: string, value: unknown) =>
            callRpc("setPaintProperty", [layer, name, value]),
        setLayoutProperty: (layer: string, name: string, value: unknown) =>
            callRpc("setLayoutProperty", [layer, name, value]),
        setFilter: (layer: string, filter?: unknown) => callRpc("setFilter", [layer, filter]),
        addLayer: (layer: unknown, beforeId?: string) => callRpc("addLayer", [layer, beforeId]),
        removeLayer: (id: string) => callRpc("removeLayer", [id]),
        setGlobalStateProperty: (name: string, value: unknown) =>
            callRpc("setGlobalStateProperty", [name, value]),
        getGlobalState: () => callRpc("getGlobalState", []),
        setGeoJsonData: (source: string, data: unknown) => coalescer.setGeoJsonData(source, data),
        updateGeoJsonData: (source: string, diff: unknown) =>
            callRpc("updateGeoJsonData", [source, diff]),
        queryRenderedFeatures: (geometry?: unknown, options?: unknown) =>
            callRpc("queryRenderedFeatures", [geometry, options]),
        remove: teardown,
        destroy: teardown,
        free: teardown,
    };

    if (ARENA_TELEMETRY) {
        proxy.setArenaCameraListener = (cb: ((camera: CameraState) => void) | null) => {
            cameraListener = cb;
            if (!closed) {
                renderWorker.postMessage({type: "listen", kind: "camera", enabled: Boolean(cb)});
            }
        };
        proxy.setArenaTelemetryListener = (cb: ((telemetry: ArenaFrameTelemetry) => void) | null) => {
            telemetryListener = cb;
            if (!closed) {
                renderWorker.postMessage({type: "listen", kind: "telemetry", enabled: Boolean(cb)});
            }
        };
        proxy.waitArenaLoaded = () => callRpc("waitArenaLoaded", []);
        proxy.waitArenaIdle = async () => {
            await coalescer.waitGeoJsonSettled();
            return callRpc("waitArenaIdle", []);
        };
    }

    return proxy as unknown as MapController;
};
