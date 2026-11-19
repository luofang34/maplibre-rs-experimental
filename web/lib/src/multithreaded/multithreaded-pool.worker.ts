import * as maplibre from "../wasm/maplibre"
import {initializeWasm} from "../wasm-instance.mjs";

type BindPortMessage = {type: 'bind_port', port: MessagePort}
type MessageData =
    | {type: 'wasm_init', module: WebAssembly.Module, memory: WebAssembly.Memory}
    | {type: 'pool_call', work_ptr: number}

let initialised: Promise<maplibre.InitOutput> | null = null;
let resolveInitReady: (() => void) | null = null;
const initReady = new Promise<void>(resolve => {
    resolveInitReady = resolve;
});

const handleMessage = async (message: MessageEvent<MessageData>) => {
    const data = message.data;
    if (!data) return;

    const type = data.type;
    if (type === 'wasm_init') {
        const module = data.module;
        const memory = data.memory;
        initialised = initializeWasm(module, memory).catch(err => {
            // Propagate to main `onerror`:
            setTimeout(() => {
                throw err;
            });
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

    if (type === 'pool_call') {
        const work_ptr = data.work_ptr; // shared memory pointer valid across render worker and pool workers

        const process_data: (msg: any) => Promise<void> = maplibre["multithreaded_process_data"]

        if (!process_data) {
            throw Error("multithreaded_worker_entry is not defined. Maybe the Rust build used the wrong build configuration.")
        }

        await process_data(work_ptr);
    }
}

onmessage = (message: MessageEvent<BindPortMessage | MessageData>) => {
    const data = message.data;
    if (!data) return;
    if (data.type === 'bind_port') {
        const port = data.port;
        (self as any).__maplibreRenderPort = port;
        port.onmessage = (ev: MessageEvent<MessageData>) => {
            void handleMessage(ev);
        };
        port.start?.();
        return;
    }
    void handleMessage(message as MessageEvent<MessageData>);
}
