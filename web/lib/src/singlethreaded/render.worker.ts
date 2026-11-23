import * as maplibre from "../wasm/maplibre";
import {initializeWasm} from "../wasm-instance.mjs";
import {ensureWebGpuCompatibility} from "../webgpu-compat.mjs";

type PreinitMessage = {
    type: "preinit"
    module: WebAssembly.Module
    memory?: WebAssembly.Memory
    canvas: OffscreenCanvas
    width: number
    height: number
    scaleFactor: number
    ports: MessagePort[]
    eagerWorkerCount?: number
}

type FinishInitMessage = {
    type: "finish_init"
    id: number
    styleJson?: string
}

type PreinitDeferredMessage = {
    type: "preinit_deferred"
    id: number
    module: WebAssembly.Module
    memory?: WebAssembly.Memory
    canvas: OffscreenCanvas
    width: number
    height: number
    scaleFactor: number
    ports: MessagePort[]
    eagerWorkerCount?: number
}

type StyleReadyMessage = {
    type: "style_ready"
    styleJson?: string
}

type StyleErrorMessage = {
    type: "style_error"
    error: string
}

type InitMessage = {
    type: "init"
    id: number
    module: WebAssembly.Module
    memory?: WebAssembly.Memory
    canvas: OffscreenCanvas
    width: number
    height: number
    scaleFactor: number
    ports: MessagePort[]
    eagerWorkerCount?: number
    styleJson?: string
}

type RenderWorkerMessage =
    | PreinitMessage
    | FinishInitMessage
    | PreinitDeferredMessage
    | StyleReadyMessage
    | StyleErrorMessage
    | InitMessage
    | { type: "resize", width: number, height: number, scaleFactor: number }
    | { type: "pmove", x: number, y: number }
    | { type: "pbtn", button: number, pressed: boolean, x: number, y: number }
    | { type: "wheel", dx: number, dy: number, mode: number, ctrl: boolean, shift: boolean, alt: boolean }
    | { type: "touch", id: number, phase: number, x: number, y: number }
    | { type: "mods", shift: boolean, alt: boolean, ctrl: boolean }
    | { type: "key", key: string, pressed: boolean }
    | { type: "listen", kind: "camera" | "telemetry", enabled: boolean }
    | { type: "rpc", id: number, method: string, args: unknown[] }
    | { type: "destroy" }

if (!WEBGL) {
    ensureWebGpuCompatibility(self, {sharedMemory: MULTITHREADED, traceGpuCalls: false});
}

let runner: any = null;
let controller: any = null;
let ready: Promise<void> | null = null;
let preinitPromise: Promise<any> | null = null;
let activeWasmMemory: WebAssembly.Memory | undefined;
let dormantPorts: Array<MessagePort | null> = [];
let destroyed = false;
let pendingStyleDeferred: {
    resolve: (styleJson?: string) => void
    reject: (reason: unknown) => void
    settled: boolean
} | null = null;

const postReply = (id: number, ok: boolean, value: unknown) => {
    postMessage(ok ? {type: "rpc_ok", id, result: value} : {type: "rpc_err", id, error: String(value)});
};

const instantiateAndStartPorts = async (
    module: WebAssembly.Module,
    ports: MessagePort[],
    eagerCount: number,
    memory?: WebAssembly.Memory,
) => {
    const wasm = await initializeWasm(module, memory, true);
    activeWasmMemory = memory ?? wasm.memory;
    if (MULTITHREADED && !memory) {
        for (let i = 0; i < eagerCount && i < ports.length; i++) {
            ports[i].postMessage({type: "wasm_init", module, memory: activeWasmMemory});
        }
    }
};

const createPortAcquirer = (
    module: WebAssembly.Module,
    ports: Array<MessagePort | null>,
    eagerWorkerCount: number | undefined,
    hasSharedMemory: boolean,
) => {
    let nextPort = 0;
    const eagerCount = MULTITHREADED
        ? Math.min(eagerWorkerCount ?? ports.length, ports.length)
        : ports.length;
    dormantPorts = ports.slice();
    return () => {
        const idx = nextPort++;
        if (idx >= ports.length) return undefined;
        const port = ports[idx];
        ports[idx] = null;
        dormantPorts[idx] = null;
        if (port && MULTITHREADED && idx >= eagerCount) {
            postMessage({type: "spawn_mt_worker", slot: idx});
            if (!hasSharedMemory && activeWasmMemory) {
                port.postMessage({type: "wasm_init", module, memory: activeWasmMemory});
            }
        }
        return port ?? undefined;
    };
};

onmessage = async (event: MessageEvent<RenderWorkerMessage>) => {
    const msg = event.data;
    if (!msg) return;

    if (msg.type === "destroy") {
        destroyed = true;
        if (pendingStyleDeferred && !pendingStyleDeferred.settled) {
            pendingStyleDeferred.settled = true;
            pendingStyleDeferred.reject(new Error("Offscreen map destroyed"));
        }
        pendingStyleDeferred = null;
        if (preinitPromise) {
            const stalePreinit = preinitPromise;
            preinitPromise = null;
            try {
                const pre = await stalePreinit;
                pre?.free?.();
            } catch {
                // Ignore preinit errors during teardown.
            }
        }
        if (ready) {
            try {
                await ready;
            } catch {
                // Ignore init errors during teardown.
            }
        }
        for (const port of dormantPorts.splice(0)) {
            port?.close();
        }
        if (controller) {
            controller.setArenaCameraListener?.(null);
            controller.setArenaTelemetryListener?.(null);
            controller.free?.();
            controller = null;
        }
        if (runner) {
            runner.destroy?.();
            runner.free?.();
            runner = null;
        }
        activeWasmMemory = undefined;
        postMessage({type: "destroyed"});
        return;
    }

    if (destroyed) return;

    if (msg.type === "style_ready") {
        if (pendingStyleDeferred && !pendingStyleDeferred.settled) {
            pendingStyleDeferred.settled = true;
            pendingStyleDeferred.resolve(msg.styleJson);
        }
        pendingStyleDeferred = null;
        return;
    }

    if (msg.type === "style_error") {
        if (pendingStyleDeferred && !pendingStyleDeferred.settled) {
            pendingStyleDeferred.settled = true;
            pendingStyleDeferred.reject(new Error(msg.error));
        }
        pendingStyleDeferred = null;
        return;
    }

    if (msg.type === "preinit_deferred") {
        let resolveStyle: (styleJson?: string) => void = () => {};
        let rejectStyle: (reason: unknown) => void = () => {};
        const stylePromise = new Promise<string | undefined>((resolve, reject) => {
            resolveStyle = resolve;
            rejectStyle = reject;
        });
        pendingStyleDeferred = {resolve: resolveStyle, reject: rejectStyle, settled: false};
        const {id, module, memory, canvas, width, height, scaleFactor, ports, eagerWorkerCount} = msg;
        const hasSharedMemory = Boolean(memory);
        const initPromise = (async () => {
            const eagerCount = MULTITHREADED
                ? Math.min(eagerWorkerCount ?? ports.length, ports.length)
                : ports.length;
            await instantiateAndStartPorts(module, ports, eagerCount, memory);
            const acquirePort = createPortAcquirer(module, ports, eagerWorkerCount, hasSharedMemory);
            runner = await (maplibre as any).run_maplibre_offscreen_deferred(
                canvas,
                width,
                height,
                scaleFactor,
                acquirePort,
                stylePromise,
            );
            controller = runner.controller();
        })();
        ready = initPromise;
        try {
            await initPromise;
            ready = null;
            if (!destroyed) postReply(id, true, undefined);
        } catch (err) {
            ready = null;
            if (!destroyed) postReply(id, false, err);
        }
        return;
    }

    if (msg.type === "preinit") {
        const {module, memory, canvas, width, height, scaleFactor, ports, eagerWorkerCount} = msg;
        const hasSharedMemory = Boolean(memory);
        preinitPromise = (async () => {
            const eagerCount = MULTITHREADED
                ? Math.min(eagerWorkerCount ?? ports.length, ports.length)
                : ports.length;
            await instantiateAndStartPorts(module, ports, eagerCount, memory);
            const acquirePort = createPortAcquirer(module, ports, eagerWorkerCount, hasSharedMemory);
            return (maplibre as any).preinit_maplibre_offscreen(
                canvas,
                width,
                height,
                scaleFactor,
                acquirePort,
            );
        })();
        preinitPromise.catch(() => {});
        return;
    }

    if (msg.type === "finish_init") {
        const initPromise = (async () => {
            const activePreinit = preinitPromise;
            preinitPromise = null;
            if (!activePreinit) {
                throw new Error("Missing preinit state for finish_init");
            }
            const pre = await activePreinit;
            runner = await (maplibre as any).finish_maplibre_offscreen(pre, msg.styleJson);
            controller = runner.controller();
        })();
        ready = initPromise;
        try {
            await initPromise;
            ready = null;
            if (!destroyed) postReply(msg.id, true, undefined);
        } catch (err) {
            ready = null;
            if (!destroyed) postReply(msg.id, false, err);
        }
        return;
    }

    if (msg.type === "init") {
        const {id, module, memory, canvas, width, height, scaleFactor, ports, eagerWorkerCount, styleJson} = msg;
        const hasSharedMemory = Boolean(memory);
        const initPromise = (async () => {
            const eagerCount = MULTITHREADED
                ? Math.min(eagerWorkerCount ?? ports.length, ports.length)
                : ports.length;
            await instantiateAndStartPorts(module, ports, eagerCount, memory);
            const acquirePort = createPortAcquirer(module, ports, eagerWorkerCount, hasSharedMemory);
            runner = await (maplibre as any).run_maplibre_offscreen(
                canvas,
                width,
                height,
                scaleFactor,
                acquirePort,
                styleJson,
            );
            controller = runner.controller();
        })();
        ready = initPromise;
        try {
            await initPromise;
            ready = null;
            if (!destroyed) postReply(id, true, undefined);
        } catch (err) {
            ready = null;
            if (!destroyed) postReply(id, false, err);
        }
        return;
    }

    if (ready) {
        try {
            await ready;
        } catch {
            if (msg.type === "rpc") {
                postReply(msg.id, false, "Offscreen renderer failed to initialize");
            }
            return;
        }
    }
    if (!runner || !controller || destroyed) {
        if (msg.type === "rpc") {
            postReply(msg.id, false, "Offscreen map is closed");
        }
        return;
    }

    switch (msg.type) {
        case "resize":
            runner.resize(msg.width, msg.height, msg.scaleFactor);
            break;
        case "pmove":
            runner.pointer_move(msg.x, msg.y);
            break;
        case "pbtn":
            runner.pointer_button(msg.button, msg.pressed, msg.x, msg.y);
            break;
        case "wheel":
            runner.mouse_wheel(msg.dx, msg.dy, msg.mode, msg.ctrl, msg.shift, msg.alt);
            break;
        case "touch":
            runner.touch(msg.id, msg.phase, msg.x, msg.y);
            break;
        case "mods":
            runner.modifiers(msg.shift, msg.alt, msg.ctrl);
            break;
        case "key":
            runner.key(msg.key, msg.pressed);
            break;
        case "listen":
            if (ARENA_TELEMETRY) {
                if (msg.kind === "camera") {
                    controller.setArenaCameraListener?.(
                        msg.enabled ? (payload: unknown) => postMessage({type: "camera", payload}) : null,
                    );
                } else if (msg.kind === "telemetry") {
                    controller.setArenaTelemetryListener?.(
                        msg.enabled ? (payload: unknown) => postMessage({type: "telemetry", payload}) : null,
                    );
                }
            }
            break;
        case "rpc": {
            try {
                const fn = controller[msg.method];
                if (typeof fn !== "function") {
                    postReply(msg.id, false, `Unknown controller method: ${msg.method}`);
                    return;
                }
                const result = await fn.apply(controller, msg.args);
                postReply(msg.id, true, result);
            } catch (err) {
                postReply(msg.id, false, err);
            }
            break;
        }
    }
};
