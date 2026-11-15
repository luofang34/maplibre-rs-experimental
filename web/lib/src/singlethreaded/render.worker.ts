import * as maplibre from "../wasm/maplibre";
import {initializeWasm} from "../wasm-instance.mjs";
import {ensureWebGpuCompatibility} from "../webgpu-compat.mjs";

type InitMessage = {
    type: "init"
    id: number
    module: WebAssembly.Module
    canvas: OffscreenCanvas
    width: number
    height: number
    scaleFactor: number
    ports: MessagePort[]
    styleJson?: string
}

type RenderWorkerMessage =
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

if (!MULTITHREADED && !WEBGL) {
    ensureWebGpuCompatibility(self);
}

let runner: any = null;
let controller: any = null;
let ready: Promise<void> | null = null;

const postReply = (id: number, ok: boolean, value: unknown) => {
    postMessage(ok ? {type: "rpc_ok", id, result: value} : {type: "rpc_err", id, error: String(value)});
};

if (!MULTITHREADED && !WEBGL) {
onmessage = async (event: MessageEvent<RenderWorkerMessage>) => {
    const msg = event.data;
    if (!msg) return;

    if (msg.type === "init") {
        const initPromise = (async () => {
            await initializeWasm(msg.module, undefined);
            let nextPort = 0;
            runner = await (maplibre as any).run_maplibre_offscreen(
                msg.canvas,
                msg.width,
                msg.height,
                msg.scaleFactor,
                () => msg.ports[nextPort++],
                msg.styleJson,
            );
            controller = runner.controller();
        })();
        ready = initPromise;
        try {
            await initPromise;
            ready = null;
            postReply(msg.id, true, undefined);
        } catch (err) {
            ready = null;
            postReply(msg.id, false, err);
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
    if (!runner || !controller) {
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
        case "destroy":
            runner.destroy();
            runner = null;
            controller = null;
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
}
