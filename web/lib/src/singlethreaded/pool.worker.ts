import * as maplibre from "../wasm/maplibre"
import {initializeWasm} from "../wasm-instance.mjs";

type MessageData = { type: 'wasm_init', module: WebAssembly.Module }
    | { type: 'kernel_config', config: string }
    | { type: 'call', procedure_ptr: number, attempt: bigint, style_revision: bigint, input: string }
    | { type: 'style', revision: bigint, style: string }
    | { type: 'cancel', attempt: bigint }
    | { type: 'image_provider_modules', urls: string[] }

let initialised: Promise<maplibre.InitOutput> = null

// Settles once `wasm_init` has started the module, which may come after the provider modules.
let markStarted: () => void
const started = new Promise<void>(resolve => markStarted = resolve)

// Tile calls wait for the host's image providers, so no label is laid out without them.
let providersRegistered: Promise<void> = Promise.resolve()

interface TileWorkerCalls {
    set_style(revision: bigint, style: string): void
    process(procedure_ptr: number, style_revision: bigint, input: string): Promise<void>
    cancel(attempt: bigint): boolean
    finish_failed(attempt: bigint): void
}

// The calls this worker runs, made once the module has started.
let calls: TileWorkerCalls = null

// Calls received but not yet started, so a cancel that arrives while one waits can drop it.
const waiting = new Map<bigint, { cancelled: boolean }>()

/** Imports each module and lets its default export register image providers in this worker. */
const registerProviders = async (urls: string[]) => {
    await started
    await initialised
    for (const url of urls) {
        const module = await import(/* @vite-ignore */ url)
        await module.default({registerImageProvider: maplibre["register_image_provider"]})
    }
}

/** The calls this worker runs, made once the module has started. */
const workerCalls = (): TileWorkerCalls => {
    if (!calls) {
        const TileWorkerCalls: new () => TileWorkerCalls = maplibre["TileWorkerCalls"];
        if (!TileWorkerCalls) {
            throw Error("TileWorkerCalls is not defined. Maybe the Rust build used the wrong build configuration.")
        }
        calls = new TileWorkerCalls()
    }
    return calls
}

onmessage = async (message: MessageEvent<MessageData>) => {
    // Calls and cancels are noted before any await: the port delivers them in order, but
    // their handlers resume in the order their awaits settle.
    let call: { cancelled: boolean } = null
    if (message.data.type === 'call') {
        call = {cancelled: false}
        waiting.set(message.data.attempt, call)
    } else if (message.data.type === 'cancel') {
        const waitingCall = waiting.get(message.data.attempt)
        if (waitingCall) {
            waitingCall.cancelled = true
            return
        }
    }

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
        waiting.delete(data.attempt)
        if (call.cancelled) {
            return
        }
        // WARNING: Do not modify data passed from Rust!
        const procedure_ptr = data.procedure_ptr;
        const input = data.input;

        try {
            await workerCalls().process(procedure_ptr, data.style_revision, input);
        } catch (err) {
            workerCalls().finish_failed(data.attempt);
            throw err;
        }
    } else if (type === 'style') {
        // Ordered behind `providersRegistered` like `call`, so queued startup calls see their
        // style revision before later style updates can evict it.
        await providersRegistered;
        workerCalls().set_style(message.data.revision, message.data.style)
    } else if (type === 'cancel') {
        calls?.cancel(message.data.attempt)
    } else if (type === 'kernel_config') {
        const data = message.data;

        const set_kernel_config: (config: string) => void = maplibre["set_kernel_config"];

        if (!set_kernel_config) {
            throw Error("set_kernel_config is not defined. Maybe the Rust build used the wrong build configuration.")
        }


        set_kernel_config(data.config)
    }
}
