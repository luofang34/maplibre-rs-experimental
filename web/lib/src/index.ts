import * as maplibre from "./wasm/maplibre"
import {initializeWasm} from "./wasm-instance.mjs";
import {Spector} from "spectorjs"
import {checkRequirements, checkWasmFeatures} from "./browser";
import {preventDefaultTouchActions} from "./canvas";
// @ts-ignore esbuild plugin is handling this
import MultithreadedPoolWorker from './multithreaded/multithreaded-pool.worker.js';
// @ts-ignore esbuild plugin is handling this
import PoolWorker from './singlethreaded/pool.worker.js';

/**
 * Starts the map. `styleJson` is a MapLibre style document; without it the built-in style is used.
 *
 * `imageProviderModules` are URLs of JavaScript modules that make the images labels name, such
 * as road shields. The tile workers of a single-threaded build import each one and call its
 * default export with `{registerImageProvider(namespace, generation, draw)}`, where `draw`
 * is `(id, pixelRatio) => image | Promise<image>` and an image is `{width, height, data,
 * pixelRatio?, anchor?}`, `null` or `{status: "absent" | "unavailable" | "failed"}`. A
 * multithreaded build shares one registry with Rust providers instead.
 */
export const startMapLibre = async (wasmPath: string | undefined, workerPath: string | undefined, styleJson?: string, imageProviderModules: string[] = []) => {
    await checkWasmFeatures()

    let message = checkRequirements();
    if (message) {
        console.error(message)
        alert(message)
        return
    }

    if (WEBGL) {
        let spector = new Spector()
        spector.displayUI()
    }

    preventDefaultTouchActions();
    await initializeWasm(wasmPath, undefined);

    if (MULTITHREADED) {
        if (imageProviderModules.length > 0) {
            console.warn("JavaScript image providers need a single-threaded build; register Rust providers through image_providers() instead.")
        }
        await maplibre.run_maplibre(() => {
            return workerPath ? new Worker(workerPath, {
                type: 'module',
            }) : MultithreadedPoolWorker();
        }, styleJson);
    } else {
        await maplibre.run_maplibre((received_ptr: number) => {
            let worker: Worker = workerPath ? new Worker(workerPath, {
                type: 'module',
            }) : PoolWorker();  // Setting a "name" for this webworker is not yet supported, because it needs support from esbuild-plugin-inline-worker

            if (imageProviderModules.length > 0) {
                // Workers do not share the page's base URL, so they get absolute ones.
                worker.postMessage({
                    type: 'image_provider_modules',
                    urls: imageProviderModules.map(url => new URL(url, location.href).href),
                })
            }

            // Handle messages coming back from the Worker
            worker.onmessage = (message: MessageEvent<[tag: number, buffer: ArrayBuffer]>) => {
                // WARNING: Do not modify data passed from Rust!
                let data = message.data;

                const receive_data: (received_ptr: number, tag: number, buffer: ArrayBuffer) => void = maplibre["singlethreaded_receive_data"];

                if (!receive_data) {
                    throw Error("singlethreaded_main_entry is not defined. Maybe the Rust build used the wrong build configuration.")
                }

                receive_data(received_ptr, data[0], data[1])
            }

            return worker;
        }, styleJson);
    }
}
