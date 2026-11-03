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
