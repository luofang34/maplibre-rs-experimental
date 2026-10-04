import * as maplibre from "../wasm/maplibre"
import {initializeWasm} from "../wasm-instance.mjs";

type MessageData = { type: 'wasm_init', module: WebAssembly.Module }
    | { type: 'kernel_config', config: string }
    | { type: 'call', procedure_ptr: number, input: string }
    | { type: 'image_provider_modules', urls: string[] }

let initialised: Promise<maplibre.InitOutput> = null

// Settles once `wasm_init` has started the module, which may come after the provider modules.
let markStarted: () => void
const started = new Promise<void>(resolve => markStarted = resolve)

// Tile calls wait for the host's image providers, so no label is laid out without them.
let providersRegistered: Promise<void> = Promise.resolve()

/** Imports each module and lets its default export register image providers in this worker. */
const registerProviders = async (urls: string[]) => {
    await started
    await initialised
    for (const url of urls) {
        const module = await import(/* @vite-ignore */ url)
        await module.default({registerImageProvider: maplibre["register_image_provider"]})
    }
}

onmessage = async (message: MessageEvent<MessageData>) => {

    if (message.data.type === 'image_provider_modules') {
        providersRegistered = registerProviders(message.data.urls).catch(err => {
            setTimeout(() => {
                throw err;
            });
        })
        return
    }

    if (initialised) {
        // This will queue further commands up until the module is fully initialised:
        await initialised;
    }

    const type = message.data.type;
    if (type === 'wasm_init') {
        const data = message.data;
        let module = data.module;
        initialised = initializeWasm(module, undefined).catch(err => {
            // Propagate to main `onerror`:
            setTimeout(() => {
                throw err;
            });
            // Rethrow to keep promise rejected and prevent execution of further commands:
            throw err;
        });
        markStarted();
    } else if (type === 'call') {
        await providersRegistered;
        const data = message.data;
        // WARNING: Do not modify data passed from Rust!
        const procedure_ptr = data.procedure_ptr;
        const input = data.input;

        const process_data: (procedure_ptr: number, input: string) => Promise<void> = maplibre["singlethreaded_process_data"];

        if (!process_data) {
            throw Error("singlethreaded_worker_entry is not defined. Maybe the Rust build used the wrong build configuration.")
        }

        await process_data(procedure_ptr, input);
    } else if (type === 'kernel_config') {
        const data = message.data;

        const set_kernel_config: (config: string) => void = maplibre["set_kernel_config"];

        if (!set_kernel_config) {
            throw Error("set_kernel_config is not defined. Maybe the Rust build used the wrong build configuration.")
        }


        set_kernel_config(data.config)
    }
}
