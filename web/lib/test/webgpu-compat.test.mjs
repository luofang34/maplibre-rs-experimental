import assert from 'node:assert/strict';
import test from 'node:test';
import {ensureWebGpuCompatibility} from '../src/webgpu-compat.mjs';

test('populates missing Safari GPUAdapterInfo subgroup and fallback fields', () => {
    class MockSafariAdapterInfo {
        get vendor() {
            return 'apple';
        }
        get subgroupMinSize() {
            return undefined;
        }
    }

    ensureWebGpuCompatibility({GPUAdapterInfo: MockSafariAdapterInfo});
    const info = new MockSafariAdapterInfo();

    assert.equal(info.vendor, 'apple');
    assert.equal(info.subgroupMinSize, 0);
    assert.equal(info.subgroupMaxSize, 0);
    assert.equal(info.isFallbackAdapter, false);
    assert.equal(info.description, '');
});

test('preserves defined numeric subgroup sizes', () => {
    class MockChromeAdapterInfo {
        get subgroupMinSize() {
            return 4;
        }
        get subgroupMaxSize() {
            return 32;
        }
        get isFallbackAdapter() {
            return true;
        }
    }

    ensureWebGpuCompatibility({GPUAdapterInfo: MockChromeAdapterInfo});
    const info = new MockChromeAdapterInfo();

    assert.equal(info.subgroupMinSize, 4);
    assert.equal(info.subgroupMaxSize, 32);
    assert.equal(info.isFallbackAdapter, true);
});

test('unshares SharedArrayBuffer views in setBindGroup and writeBuffer', () => {
    if (typeof SharedArrayBuffer === 'undefined') {
        return;
    }
    const calls = [];
    class MockQueue {
        writeBuffer(buf, offset, data, dataOffset, size) {
            assert.equal(data.buffer instanceof SharedArrayBuffer, false);
            calls.push({op: 'writeBuffer', bytes: Array.from(new Uint8Array(data.buffer, data.byteOffset, size))});
        }
    }
    class MockRenderPass {
        setBindGroup(index, group, offsets, start, length) {
            if (ArrayBuffer.isView(offsets)) {
                assert.equal(offsets.buffer instanceof SharedArrayBuffer, false);
                calls.push({op: 'setBindGroup', index, vals: Array.from(offsets.subarray(start, start + length))});
            }
        }
    }

    ensureWebGpuCompatibility({
        SharedArrayBuffer,
        GPUQueue: MockQueue,
        GPURenderPassEncoder: MockRenderPass,
    });

    const sab = new SharedArrayBuffer(32);
    const u32 = new Uint32Array(sab);
    u32.set([256, 512, 1024], 1);

    const pass = new MockRenderPass();
    pass.setBindGroup(0, {}, u32, 1, 2);

    const u8 = new Uint8Array(sab);
    u8.set([10, 20, 30, 40], 4);
    const queue = new MockQueue();
    queue.writeBuffer({}, 0, u8, 4, 4);

    assert.deepEqual(calls, [
        {op: 'setBindGroup', index: 0, vals: [256, 512]},
        {op: 'writeBuffer', bytes: [10, 20, 30, 40]},
    ]);
});

test('forwards 2-argument setBindGroup without passing undefined dynamicOffsets', () => {
    const argCounts = [];
    class MockRenderPass2Arg {
        setBindGroup() {
            argCounts.push(arguments.length);
            if (arguments.length === 3 && arguments[2] === undefined) {
                throw new TypeError('The provided value cannot be converted to a sequence.');
            }
        }
    }

    ensureWebGpuCompatibility({
        GPURenderPassEncoder: MockRenderPass2Arg,
    });

    const pass = new MockRenderPass2Arg();
    pass.setBindGroup(0, {});
    pass.setBindGroup(1, {}, undefined);
    pass.setBindGroup(2, {}, [0]);

    assert.deepEqual(argCounts, [2, 2, 3]);
});

test('bounds writeTexture SharedArrayBuffer copy to required texture slice', () => {
    if (typeof SharedArrayBuffer === 'undefined') {
        return;
    }
    const calls = [];
    class MockTextureQueue {
        writeTexture(destination, data, dataLayout, size) {
            assert.equal(data.buffer instanceof SharedArrayBuffer, false);
            calls.push({
                byteLength: data.byteLength,
                firstByte: data[0],
                lastByte: data[data.byteLength - 1],
                offset: dataLayout?.offset ?? 0,
                size,
            });
        }
    }

    ensureWebGpuCompatibility({
        SharedArrayBuffer,
        GPUQueue: MockTextureQueue,
    });

    const sab = new SharedArrayBuffer(16 * 1024 * 1024);
    const view = new Uint8Array(sab);
    view[4096] = 0xab;
    view[4096 + 4095] = 0xcd;

    const queue = new MockTextureQueue();
    queue.writeTexture(
        {},
        sab,
        {offset: 4096, bytesPerRow: 256},
        {width: 32, height: 16, depthOrArrayLayers: 1},
    );

    assert.equal(calls.length, 1);
    assert.equal(calls[0].byteLength, 4096);
    assert.equal(calls[0].firstByte, 0xab);
    assert.equal(calls[0].lastByte, 0xcd);
    assert.equal(calls[0].offset, 0);
});

test('reuses unshared scratch ArrayBuffer across repeated writeBuffer and writeTexture calls', () => {
    if (typeof SharedArrayBuffer === 'undefined') {
        return;
    }
    const seenBuffers = [];
    class MockReuseQueue {
        writeBuffer(buf, offset, data, dataOffset, size) {
            seenBuffers.push({buffer: data.buffer, size});
        }
        writeTexture(destination, data) {
            seenBuffers.push({buffer: data.buffer, size: data.byteLength});
        }
    }

    ensureWebGpuCompatibility({
        SharedArrayBuffer,
        GPUQueue: MockReuseQueue,
    });

    const sab = new SharedArrayBuffer(32768);
    const u8 = new Uint8Array(sab);
    const queue = new MockReuseQueue();
    queue.writeBuffer({}, 0, u8, 0, 1024);
    queue.writeBuffer({}, 0, u8, 1024, 4096);
    queue.writeTexture({}, u8.subarray(0, 8192), {bytesPerRow: 256}, {width: 32, height: 32, depthOrArrayLayers: 1});

    assert.equal(seenBuffers.length, 3);
    assert.equal(seenBuffers[0].buffer, seenBuffers[1].buffer);
    assert.equal(seenBuffers[1].buffer, seenBuffers[2].buffer);
});
