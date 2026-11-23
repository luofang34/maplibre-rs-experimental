import test from 'node:test';
import assert from 'node:assert/strict';

import {
    WORDS_PER_CALL,
    decodeBatchAttempt,
    filterBatchWords,
    registerBatchCalls,
} from '../src/singlethreaded/st-call-batch.mjs';

function packCalls(calls) {
    const out = new Uint32Array(calls.length * WORDS_PER_CALL);
    const view = new DataView(out.buffer);
    calls.forEach((c, idx) => {
        const base = idx * WORDS_PER_CALL;
        out[base] = Number(c.attempt & 0xffff_ffffn);
        out[base + 1] = Number((c.attempt >> 32n) & 0xffff_ffffn);
        out[base + 2] = Number(c.styleRevision & 0xffff_ffffn);
        out[base + 3] = Number((c.styleRevision >> 32n) & 0xffff_ffffn);
        view.setInt32((base + 4) * 4, c.x, true);
        view.setInt32((base + 5) * 4, c.y, true);
        out[base + 6] = c.meta >>> 0;
        view.setFloat32((base + 7) * 4, c.pixelRatio, true);
    });
    return out;
}

test('decodeBatchAttempt preserves unsigned 64-bit high-bit values', () => {
    const attempt = (1n << 63n) | 0x1234_5678_9abcn;
    const words = packCalls([
        {attempt, styleRevision: 7n, x: -5, y: 12, meta: 0x01040201, pixelRatio: 1.5},
    ]);
    assert.equal(decodeBatchAttempt(words, 0), attempt);
});

test('registerBatchCalls and filterBatchWords honor cancels arriving before awaiting init', () => {
    const waiting = new Map();
    const words = packCalls([
        {attempt: 11n, styleRevision: 3n, x: 1, y: 2, meta: 100, pixelRatio: 1},
        {attempt: 12n, styleRevision: 3n, x: 3, y: 4, meta: 101, pixelRatio: 2},
        {attempt: 13n, styleRevision: 3n, x: 5, y: 6, meta: 102, pixelRatio: 1.25},
    ]);

    const reg = registerBatchCalls(words, waiting);
    assert.equal(waiting.size, 3);

    // Cancel arrives before awaiting initialization settles.
    waiting.get(12n).cancelled = true;

    const filtered = filterBatchWords(words, reg, waiting);
    assert.equal(waiting.size, 0);
    assert.ok(filtered instanceof Uint32Array);
    assert.equal(filtered.length, 2 * WORDS_PER_CALL);
    assert.equal(decodeBatchAttempt(filtered, 0), 11n);
    assert.equal(decodeBatchAttempt(filtered, WORDS_PER_CALL), 13n);
});

test('filterBatchWords returns original buffer when none cancelled and null when all cancelled', () => {
    const waiting = new Map();
    const words = packCalls([
        {attempt: 21n, styleRevision: 1n, x: 0, y: 0, meta: 1, pixelRatio: 1},
        {attempt: 22n, styleRevision: 1n, x: 1, y: 0, meta: 1, pixelRatio: 1},
    ]);

    const reg1 = registerBatchCalls(words, waiting);
    assert.equal(filterBatchWords(words, reg1, waiting), words);

    const reg2 = registerBatchCalls(words, waiting);
    waiting.get(21n).cancelled = true;
    waiting.get(22n).cancelled = true;
    assert.equal(filterBatchWords(words, reg2, waiting), null);
    assert.equal(waiting.size, 0);
});
