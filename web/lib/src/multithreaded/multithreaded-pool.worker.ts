import * as maplibre from "../wasm/maplibre"
import {initializeWasm} from "../wasm-instance.mjs";

type BindPortMessage = {type: 'bind_port', port: MessagePort}
type MessageData =
    | {type: 'wasm_init', module: WebAssembly.Module, memory: WebAssembly.Memory}
    | {type: 'pool_call', work_ptr: number}
    | number

let initialised: Promise<maplibre.InitOutput> | null = null;
let resolveInitReady: (() => void) | null = null;
const initReady = new Promise<void>(resolve => {
    resolveInitReady = resolve;
});
let wasmReady = false;
let renderPort: MessagePort | null = null;
let portReadySent = false;
let globalReadySent = false;

const notifyReady = () => {
    if (!wasmReady) return;
    if (renderPort) {
        if (!portReadySent) {
            portReadySent = true;
            renderPort.postMessage({type: 'worker_ready'});
        }
        return;
    }
    if (!globalReadySent) {
        globalReadySent = true;
        self.postMessage({type: 'worker_ready'});
    }
};

const handleMessage = async (message: MessageEvent<MessageData>) => {
    const data = message.data;
    if (data === null || data === undefined) return;

    if (typeof data !== 'number' && data.type === 'wasm_init') {
        if (initialised) return;
        const module = data.module;
        const memory = data.memory;
        initialised = initializeWasm(module, memory)
            .then(output => {
                wasmReady = true;
                notifyReady();
                return output;
            })
            .catch(err => {
                // Propagate uncaught rejection without unhandled rejection races:
                globalThis.reportError?.(err);
                // Rethrow to keep promise rejected and prevent execution of further commands:
                throw err;
            });
        resolveInitReady?.();
        resolveInitReady = null;
        return;
    }

    if (!initialised) {
        await initReady;
    }
    // Queue further commands up until the module is fully initialised:
    await initialised;

    const work_ptr = typeof data === 'number'
        ? data
        : data.type === 'pool_call'
            ? data.work_ptr
            : null;
    if (work_ptr === null) return;

    const process_data: (msg: any) => Promise<void> = maplibre["multithreaded_process_data"]

    if (!process_data) {
        throw Error("multithreaded_worker_entry is not defined. Maybe the Rust build used the wrong build configuration.")
    }

    await process_data(work_ptr);
}

onmessage = (message: MessageEvent<BindPortMessage | MessageData>) => {
    const data = message.data;
    if (data === null || data === undefined) return;
    if (typeof data !== 'number' && data.type === 'bind_port') {
        const port = data.port;
        renderPort = port;
        (self as any).__maplibreRenderPort = port;
        port.onmessage = (ev: MessageEvent<MessageData>) => {
            void handleMessage(ev);
        };
        port.start?.();
        notifyReady();
        return;
    }
    void handleMessage(message as MessageEvent<MessageData>);
}
