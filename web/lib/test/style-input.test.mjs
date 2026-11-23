import assert from "node:assert/strict";
import test from "node:test";
import { normalizeStyleInput } from "../src/style-input.mjs";

test("normalizeStyleInput keeps undefined, null, and JSON strings synchronous without fetching", async () => {
    let fetched = 0;
    const fetchImpl = async () => {
        fetched++;
        return { ok: true, status: 200, text: async () => "{}" };
    };

    const undef = normalizeStyleInput(undefined, undefined, { fetchImpl });
    assert.equal(undef.resolved, true);
    assert.equal(undef.value, undefined);
    assert.equal(await undef.promise, undefined);

    const nil = normalizeStyleInput(null, undefined, { fetchImpl });
    assert.equal(nil.resolved, true);
    assert.equal(nil.value, undefined);
    assert.equal(await nil.promise, undefined);

    const inlineJson = '  {"version":8,"sources":{},"layers":[]}';
    const jsonRes = normalizeStyleInput(inlineJson, undefined, { fetchImpl });
    assert.equal(jsonRes.resolved, true);
    assert.equal(jsonRes.value, inlineJson);
    assert.equal(await jsonRes.promise, inlineJson);

    const objRes = normalizeStyleInput({ version: 8, layers: [] }, undefined, { fetchImpl });
    assert.equal(objRes.resolved, true);
    assert.equal(objRes.value, '{"version":8,"layers":[]}');
    assert.equal(await objRes.promise, '{"version":8,"layers":[]}');

    assert.equal(fetched, 0);
});

test("normalizeStyleInput fetches style URL immediately, applies transformRequest, and avoids double-fetch", async () => {
    const calls = [];
    const fetchImpl = async (url, init) => {
        calls.push({ url, init });
        return {
            ok: true,
            status: 200,
            text: async () => '{"version":8,"name":"fetched"}',
        };
    };

    const res = normalizeStyleInput(
        "https://example.test/style.json",
        (url, type) => {
            assert.equal(type, "Style");
            return {
                url: `${url}?token=abc`,
                headers: { Authorization: "Bearer test" },
                credentials: "omit",
            };
        },
        { fetchImpl },
    );

    assert.equal(res.resolved, false);
    assert.equal(calls.length, 1);
    assert.equal(calls[0].url, "https://example.test/style.json?token=abc");
    assert.deepEqual(calls[0].init, {
        headers: { Authorization: "Bearer test" },
        credentials: "omit",
    });
    assert.equal(await res.promise, '{"version":8,"name":"fetched"}');
    assert.equal(calls.length, 1);
});

test("normalizeStyleInput supports Promise<string | object> and surfaces HTTP errors", async () => {
    const promisedObj = normalizeStyleInput(Promise.resolve({ version: 8 }));
    assert.equal(promisedObj.resolved, false);
    assert.equal(await promisedObj.promise, '{"version":8}');

    const errRes = normalizeStyleInput("https://example.test/missing.json", undefined, {
        fetchImpl: async () => ({ ok: false, status: 404, text: async () => "" }),
    });
    await assert.rejects(errRes.promise, /HTTP 404/);
});
