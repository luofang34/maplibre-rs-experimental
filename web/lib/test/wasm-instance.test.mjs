import assert from 'node:assert/strict';
import {once} from 'node:events';
import {readFile} from 'node:fs/promises';
import test from 'node:test';
import {isMainThread, parentPort, workerData, Worker} from 'node:worker_threads';
import {initializeWasm} from '../src/wasm-instance.mjs';

if (!isMainThread) {
    const instance = await initializeWasm(workerData.module, workerData.memory);
    parentPort.postMessage({
        reusesMemory: instance.memory === workerData.memory,
        shared: instance.memory.buffer instanceof SharedArrayBuffer,
        value: workerData.pointer === undefined ? undefined :
            new Uint32Array(instance.memory.buffer)[workerData.pointer / 4],
    });
} else {
    const bytes = await readFile(new URL('../src/wasm/maplibre_bg.wasm', import.meta.url));
    const module = await WebAssembly.compile(bytes);
    const instance = await initializeWasm(module, undefined);
    const shared = instance.memory.buffer instanceof SharedArrayBuffer;

    test('generated memory limits allow main-thread initialization', () => {
        assert.ok(instance.memory instanceof WebAssembly.Memory);
        assert.ok(instance.memory.buffer.byteLength > 0);
    });

    test('workers initialize with the module and correct memory ownership', async () => {
        const pointer = shared ? instance.__wbindgen_malloc(4, 4) : undefined;
        if (shared) new Uint32Array(instance.memory.buffer)[pointer / 4] = 0x12345678;
        const worker = new Worker(new URL(import.meta.url), {
            workerData: {module, memory: shared ? instance.memory : undefined, pointer},
        });
        try {
            const [result] = await once(worker, 'message');
            assert.equal(result.shared, shared);
            assert.equal(result.reusesMemory, shared);
            if (shared) assert.equal(result.value, 0x12345678);
        } finally {
            await worker.terminate();
            if (shared) instance.__wbindgen_free(pointer, 4, 4);
        }
    });
}
