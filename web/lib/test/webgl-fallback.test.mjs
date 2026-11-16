import assert from 'node:assert/strict';
import test from 'node:test';
import {
    probeWebGpuAvailable,
    resetWebGpuProbeForTest,
    selectGraphicsBackend,
} from '../src/browser.ts';

test('selectGraphicsBackend chooses WebGL2 fallback when WebGPU is absent and WebGL2 is available', () => {
    assert.deepEqual(
        selectGraphicsBackend({
            webgl: false,
            hasWebGpu: false,
            hasWebGl: true,
            hasWebglFallbackModule: true,
        }),
        {backend: 'fallback-webgl2'},
    );
});

test('selectGraphicsBackend chooses WebGPU when available', () => {
    assert.deepEqual(
        selectGraphicsBackend({
            webgl: false,
            hasWebGpu: true,
            hasWebGl: true,
            hasWebglFallbackModule: true,
        }),
        {backend: 'webgpu'},
    );
});

test('selectGraphicsBackend returns typed error when neither WebGPU nor WebGL2 works', () => {
    const decision = selectGraphicsBackend({
        webgl: false,
        hasWebGpu: false,
        hasWebGl: false,
        hasWebglFallbackModule: true,
    });
    assert.equal(decision.backend, 'error');
});

test('probeWebGpuAvailable handles missing navigator.gpu or null adapter without throwing', async () => {
    resetWebGpuProbeForTest();
    assert.equal(await probeWebGpuAvailable(undefined), false);
    assert.equal(await probeWebGpuAvailable({}), false);
    assert.equal(
        await probeWebGpuAvailable({gpu: {requestAdapter: async () => null}}),
        false,
    );
    assert.equal(
        await probeWebGpuAvailable({gpu: {requestAdapter: async () => { throw new Error('denied'); }}}),
        false,
    );
    assert.equal(
        await probeWebGpuAvailable({gpu: {requestAdapter: async () => ({features: new Set()})}}),
        true,
    );
});
