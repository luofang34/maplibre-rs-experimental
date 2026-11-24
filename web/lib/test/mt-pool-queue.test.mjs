import assert from "node:assert/strict";
import {spawnSync} from "node:child_process";
import {mkdtempSync, rmSync, writeFileSync} from "node:fs";
import {tmpdir} from "node:os";
import path from "node:path";
import test from "node:test";
import {fileURLToPath} from "node:url";

// The pool module only compiles for `wasm32 + atomics`, which no Rust test run targets, so the
// dispatch queue's unit tests are compiled for the host here.
const queueSource = fileURLToPath(
    new URL("../../src/platform/multithreaded/pool/queue.rs", import.meta.url),
);

test("MT pool dispatch queue unit tests pass on the host", () => {
    const dir = mkdtempSync(path.join(tmpdir(), "mt-pool-queue-"));
    try {
        const wrapper = path.join(dir, "wrapper.rs");
        const binary = path.join(dir, "queue-tests");
        writeFileSync(
            wrapper,
            `#![allow(dead_code)]\nmod pool { #[path = ${JSON.stringify(queueSource)}] mod queue; }\n`,
        );
        const build = spawnSync("rustc", ["--edition", "2021", "--test", wrapper, "-o", binary], {
            encoding: "utf8",
        });
        assert.equal(build.status, 0, build.stderr);
        const run = spawnSync(binary, [], {encoding: "utf8"});
        assert.equal(run.status, 0, run.stdout + run.stderr);
        assert.match(run.stdout, /test result: ok\. [1-9]\d* passed/);
    } finally {
        rmSync(dir, {recursive: true, force: true});
    }
});
