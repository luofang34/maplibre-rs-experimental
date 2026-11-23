import * as maplibre from "../wasm/maplibre_tile_worker"
import {initializeWorkerWasm} from "../wasm-worker-instance.mjs";

type BatchAttempts = ArrayLike<bigint> & Iterable<bigint>

type CallMessage =
    | { type: 'call', procedure_tag: number, attempt: bigint, style_revision: bigint, input: string }
    | {
        type: 'call',
        attempt: bigint,
        style_revision: bigint,
        x: number,
        y: number,
        meta: number,
        pixel_ratio: number,
        input?: undefined,
    }

type MessageData = { type: 'wasm_init', module: WebAssembly.Module, port?: MessagePort }
    | { type: 'kernel_config', config: string }
    | CallMessage
    | { type: 'style', revision: bigint, style: string, prefetch?: boolean }
    | { type: 'cancel', attempt: bigint }
    | { type: 'cancel_batch', attempts: BatchAttempts }
    | { type: 'image_provider_modules', urls: string[] }

let initialised: Promise<maplibre.InitOutput> = null

// Settles once `wasm_init` has started the module, which may come after the provider modules.
let markStarted: () => void
const started = new Promise<void>(resolve => markStarted = resolve)

// Tile calls wait for the host's image providers, so no label is laid out without them.
let providersRegistered: Promise<void> = Promise.resolve()

interface TileWorkerCalls {
    set_style(revision: bigint, style: string, prefetch?: boolean): void
    process(procedure_tag: number, style_revision: bigint, input: string): Promise<void>
    process_tile(
        attempt: bigint,
        style_revision: bigint,
        x: number,
        y: number,
        meta: number,
        pixel_ratio: number,
    ): Promise<void>
    cancel(attempt: bigint): boolean
    cancel_batch(attempts: BatchAttempts): void
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
        const TileWorkerCallsCtor = maplibre["TileWorkerCalls"] as unknown as new () => TileWorkerCalls;
        if (!TileWorkerCallsCtor) {
            throw Error("TileWorkerCalls is not defined. Maybe the Rust build used the wrong build configuration.")
        }
        calls = new TileWorkerCallsCtor()
    }
    return calls
}

const handleMessage = async (message: MessageEvent<MessageData>) => {
    // Calls and cancels are noted before any await: the port delivers them in order, but
    // their handlers resume in the order their awaits settle.
    let call: { cancelled: boolean } = null
    let batchCancels: bigint[] = null
    if (message.data.type === 'call') {
        call = {cancelled: false}
        waiting.set(message.data.attempt, call)
    } else if (message.data.type === 'cancel') {
        const waitingCall = waiting.get(message.data.attempt)
        if (waitingCall) {
            waitingCall.cancelled = true
            return
        }
    } else if (message.data.type === 'cancel_batch') {
        for (const attempt of message.data.attempts) {
            const waitingCall = waiting.get(attempt)
            if (waitingCall) {
                waitingCall.cancelled = true
            } else {
                (batchCancels ??= []).push(attempt)
            }
        }
        if (!batchCancels) {
            return
        }
    }

    if (message.data.type === 'image_provider_modules') {
        providersRegistered = registerProviders(message.data.urls).catch(err => {
            globalThis.reportError?.(err);
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
        const port = data.port;
        let module = data.module;
        initialised = initializeWorkerWasm(module, undefined).then(out => {
            if (port) {
                (maplibre as any).bind_worker_reply_port?.(port);
            }
            return out;
        }).catch(err => {
            // Propagate failure to host `onerror`:
            globalThis.reportError?.(err);
            // Rethrow to keep promise rejected and prevent execution of further commands:
            throw err;
        });
        if (port) {
            port.onmessage = handleMessage;
        }
        markStarted();
    } else if (type === 'call') {
        await providersRegistered;
        const data = message.data;
        waiting.delete(data.attempt)
        if (call.cancelled) {
            return
        }
        // WARNING: Do not modify data passed from Rust!
        try {
            if (typeof data.input === 'string') {
                const jsonCall = data as Extract<CallMessage, { input: string }>;
                await workerCalls().process(jsonCall.procedure_tag, jsonCall.style_revision, jsonCall.input);
            } else {
                const flat = data as Extract<CallMessage, { x: number }>;
                await workerCalls().process_tile(
                    flat.attempt,
                    flat.style_revision,
                    flat.x,
                    flat.y,
                    flat.meta,
                    flat.pixel_ratio,
                );
            }
        } catch (err) {
            workerCalls().finish_failed(data.attempt);
            throw err;
        }
    } else if (type === 'style') {
        // Ordered behind `providersRegistered` like `call`, so queued startup calls see their
        // style revision before later style updates can evict it.
        await providersRegistered;
        workerCalls().set_style(message.data.revision, message.data.style, message.data.prefetch ?? true)
    } else if (type === 'cancel') {
        calls?.cancel(message.data.attempt)
    } else if (type === 'cancel_batch') {
        if (batchCancels && calls) {
            calls.cancel_batch(batchCancels);
        }
    } else if (type === 'kernel_config') {
        const data = message.data;

        const set_kernel_config: (config: string) => void = maplibre["set_kernel_config"];

        if (!set_kernel_config) {
            throw Error("set_kernel_config is not defined. Maybe the Rust build used the wrong build configuration.")
        }


        set_kernel_config(data.config)
    }
}

onmessage = handleMessage;
