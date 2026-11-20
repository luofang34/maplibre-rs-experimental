import {build} from 'esbuild';
import metaUrlPlugin from '@chialab/esbuild-plugin-meta-url';
import inlineWorker from 'esbuild-plugin-inline-worker';
import yargs from "yargs";
import process from "process";
import {spawnSync} from "child_process"
import {existsSync, closeSync, openSync, readFileSync, renameSync, rmSync, writeFileSync} from "fs";
import {dirname} from "path";
import {fileURLToPath} from "url";
import {wasmTargetConfig} from "./build-flags.mjs";

let argv = yargs(process.argv.slice(2))
    .strict(true)
    .option('release', {
        type: 'boolean',
        description: 'Release mode'
    })
    .option('webgl', {
        type: 'boolean',
        description: 'Enable webgl'
    })
    .option('multithreaded', {
        type: 'boolean',
        description: 'Enable multithreaded support'
    })
    .option('arena-telemetry', {
        type: 'boolean',
        description: 'Enable compile-time arena telemetry hooks'
    })
    .option('esm', {
        type: 'boolean',
        description: 'Enable esm'
    })
    .option('cjs', {
        type: 'boolean',
        description: 'Enable cjs'
    })
    .option('iife', {
        type: 'boolean',
        description: 'Enable iife'
    })
    .parse()

let esm = argv.esm;
let iife = argv.iife;
let cjs = argv.cjs;
let release = argv.release;
let multithreaded = argv.multithreaded;
let arenaTelemetry = Boolean(argv.arenaTelemetry);

if (!esm && !iife && !cjs) {
    console.warn("Enabling ESM bundling as no other bundle is enabled.")
    esm = true;
}

let webgl = argv.webgl;

if (webgl) {
    console.log("WebGL support enabled.")
}

if (multithreaded) {
    console.log("multithreaded support enabled.")
}

let baseConfig = {
    platform: "browser",
    bundle: true,
    minify: release,
    assetNames: "assets/[name]",
    define: {
        WEBGL: `${webgl}`,
        MULTITHREADED: `${multithreaded}`,
        ARENA_TELEMETRY: `${arenaTelemetry}`,
        MT_INITIAL_MEMORY_PAGES: "32",
        MT_MAX_MEMORY_PAGES: "16384",
    },
}

let config = {
    ...baseConfig,
    entryPoints: ['src/index.ts'],
    plugins: [
        inlineWorker({
            ...baseConfig,
            format: "cjs",
            target: 'es2022',
            // workerName: 'worker' Supported when the follow commit is released: https://github.com/mitschabaude/esbuild-plugin-inline-worker/commit/d1aaffc721a62a3fe33f59f8f69b462c7dd05f45
        }),
        metaUrlPlugin()
    ],
};

/***
 * @returns {string} The path to the project
 */
const getProjectDirectory = () => {
    return `${getWebDirectory()}/..`
}

/***
 * @returns {string} The path to <project>/web
 */
const getWebDirectory = () => {
    return `${getLibDirectory()}/..`
}

/***
 * @returns {string} The path to <project>/web/lib
 */
const getLibDirectory = () => {
    return dirname(fileURLToPath(import.meta.url))
}

const emitTypeScript = () => {
    let outDirectory = `${getLibDirectory()}/dist/ts`;

    let child = spawnTool('npm', ["exec",
        "tsc",
        "--",
        "-m", "es2022",
        "-outDir", outDirectory,
        "--declaration",
        "--emitDeclarationOnly"
    ]);

    if (child.status !== 0) {
        throw new Error("Failed to execute tsc")
    }
}

const spawnTool = (program, args) => {
    console.debug(`Executing: ${program} ${args.join(" ")}`)
    return spawnSync(program, args, {
        cwd: '.',
        stdio: 'inherit',
    })
}

const checkWasmBindgenVersion = () => {
    const manifest = readFileSync(`${getProjectDirectory()}/Cargo.toml`, 'utf8');
    const required = manifest.match(/^wasm-bindgen\s*=\s*"=([^"]+)"/m)?.[1];
    if (!required) {
        throw new Error('Cargo.toml must pin an exact wasm-bindgen version');
    }
    const cli = spawnSync('wasm-bindgen', ['--version'], {encoding: 'utf8'});
    if (cli.status !== 0 || cli.stdout.trim() !== `wasm-bindgen ${required}`) {
        throw new Error(`Install wasm-bindgen-cli ${required} to match Cargo.toml`);
    }
}

const wasmPack = () => {
    checkWasmBindgenVersion();
    let outDirectory = `${getLibDirectory()}/src/wasm`;
    let profile = release ? (multithreaded ? "wasm-mt-release" : "wasm-release") : "wasm-dev"

    // language=toml
    let singlethreaded_config = wasmTargetConfig({release, webgl, multithreaded: false});
    let worker_singlethreaded_config = wasmTargetConfig({release, webgl, multithreaded: false, worker: true});
    // language=toml
    let multithreaded_config = wasmTargetConfig({release, webgl, multithreaded: true});



    spawnTool('cargo', ["--version"])

    const enabledFeatures = [
        "renderer",
        webgl ? "web-webgl" : "webgpu",
        ...(arenaTelemetry ? ["arena-telemetry"] : []),
    ].join(",");

    let cargo = spawnTool('cargo', [
        "--config", multithreaded ? multithreaded_config : singlethreaded_config,
        "build",
        "-p", "web", "--lib",
        "--target", "wasm32-unknown-unknown",
        "--profile", profile,
        "--no-default-features",
        "--features", enabledFeatures,
        ...(release || multithreaded
            ? ["-Z", "build-std=std,panic_abort", "-Z", "build-std-features=optimize_for_size"]
            : []),
    ]);

    if (cargo.status !== 0) {
        throw new Error("Failed to execute cargo build")
    }

    let wasmbindgen = spawnTool('wasm-bindgen', [
        `${getProjectDirectory()}/target/wasm32-unknown-unknown/${profile}/web.wasm`,
        "--out-name", "maplibre",
        "--out-dir", outDirectory,
        "--typescript",
        "--target", "web",
        ...(release ? [] : ["--debug"]),
    ]);

    if (wasmbindgen.status !== 0) {
        throw new Error("Failed to execute wasm-bindgen")
    }

    if (!multithreaded) {
        let workerProfile = release ? "wasm-worker-release" : profile;
        let workerCargo = spawnTool('cargo', [
            "--config", worker_singlethreaded_config,
            "build",
            "-p", "web-tile-worker", "--lib",
            "--target", "wasm32-unknown-unknown",
            "--profile", workerProfile,
            ...(release
                ? ["-Z", "build-std=std,panic_abort", "-Z", "build-std-features=optimize_for_size"]
                : []),
        ]);
        if (workerCargo.status !== 0) {
            throw new Error("Failed to execute cargo build for web-tile-worker")
        }

        let workerBindgen = spawnTool('wasm-bindgen', [
            `${getProjectDirectory()}/target/wasm32-unknown-unknown/${workerProfile}/web_tile_worker.wasm`,
            "--out-name", "maplibre_tile_worker",
            "--out-dir", outDirectory,
            "--typescript",
            "--target", "web",
            ...(release ? [] : ["--debug"]),
        ]);
        if (workerBindgen.status !== 0) {
            throw new Error("Failed to execute wasm-bindgen for web-tile-worker")
        }

        const workerJs = readFileSync(`${outDirectory}/maplibre_tile_worker.js`, 'utf8');
        if (workerJs.includes('GPUDevice') || workerJs.includes('GPURenderPassEncoder')) {
            throw new Error('Render-free web-tile-worker pulled WebGPU symbols into maplibre_tile_worker.js');
        }
        const mainJs = readFileSync(`${outDirectory}/maplibre.js`, 'utf8');
        if (mainJs.includes('TileWorkerCalls')) {
            throw new Error('Single-threaded main maplibre.js unexpectedly exports TileWorkerCalls');
        }
    } else {
        writeFileSync(
            `${outDirectory}/maplibre_tile_worker.js`,
            'export default async function init() { return {}; }\nexport function register_image_provider() {}\nexport function bind_worker_reply_port() {}\nexport function set_kernel_config() {}\nexport class TileWorkerCalls { set_style() {} async process() {} cancel() { return false; } finish_failed() {} }\n',
        );
        writeFileSync(
            `${outDirectory}/maplibre_tile_worker.d.ts`,
            'export type InitOutput = {};\nexport default function init(input?: unknown): Promise<InitOutput>;\nexport function register_image_provider(...args: unknown[]): void;\nexport function bind_worker_reply_port(...args: unknown[]): void;\nexport function set_kernel_config(...args: unknown[]): void;\nexport class TileWorkerCalls { set_style(revision: bigint, style: string): void; process(procedure_tag: number, style_revision: bigint, input: string): Promise<void>; cancel(attempt: bigint): boolean; finish_failed(attempt: bigint): void; }\n',
        );
        writeFileSync(`${outDirectory}/maplibre_tile_worker_bg.wasm`, Buffer.from([0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]));
    }

    if (multithreaded) {
        const mainJs = readFileSync(`${outDirectory}/maplibre.js`, 'utf8');
        const memMatch = mainJs.match(
            /new WebAssembly\.Memory\(\{\s*initial:\s*(\d+)(?:\s*,\s*maximum:\s*(\d+))?/,
        );
        if (memMatch) {
            baseConfig.define.MT_INITIAL_MEMORY_PAGES = memMatch[1];
            if (memMatch[2]) {
                baseConfig.define.MT_MAX_MEMORY_PAGES = memMatch[2];
            }
        }
    }

    const memoryTests = spawnTool('node', [
        '--test',
        'test/wasm-instance.test.mjs',
        'test/webgpu-compat.test.mjs',
        'test/offscreen-coalesce.test.mjs',
        'test/webgl-fallback.test.mjs',
    ]);
    if (memoryTests.status !== 0) {
        throw new Error('Wasm instance initialization tests failed');
    }

    if (release) {
        console.log("Running wasm-opt")
        const optArgs = (inputPath, tmpPath, optLevel = "-Oz") => [
            "exec",
            "wasm-opt", "--",
            inputPath,
            "-o", tmpPath,
            optLevel,
            "--converge",
            "--one-caller-inline-max-function-size=20",
            "--strip-debug",
            "--strip-producers",
            "--enable-bulk-memory",
            "--enable-mutable-globals",
            "--enable-nontrapping-float-to-int",
            "--enable-sign-ext",
            ...(multithreaded ? ["--enable-threads"] : ["--low-memory-unused", "--zero-filled-memory"]),
        ];

        const tmpWasm = `${outDirectory}/maplibre_bg.wasm.${process.pid}.tmp`;
        let wasmOpt = spawnTool(
            'npm',
            optArgs(`${outDirectory}/maplibre_bg.wasm`, tmpWasm, "-Oz"),
        );
        if (wasmOpt.status !== 0) {
            rmSync(tmpWasm, {force: true});
            throw new Error("Failed to execute wasm-opt")
        }
        renameSync(tmpWasm, `${outDirectory}/maplibre_bg.wasm`);

        if (!multithreaded) {
            const tmpWorkerWasm = `${outDirectory}/maplibre_tile_worker_bg.wasm.${process.pid}.tmp`;
            let workerOpt = spawnTool(
                'npm',
                optArgs(`${outDirectory}/maplibre_tile_worker_bg.wasm`, tmpWorkerWasm, "-O2"),
            );
            if (workerOpt.status !== 0) {
                rmSync(tmpWorkerWasm, {force: true});
                throw new Error("Failed to execute wasm-opt for maplibre_tile_worker_bg.wasm")
            }
            renameSync(tmpWorkerWasm, `${outDirectory}/maplibre_tile_worker_bg.wasm`);
        }

        const postOptTests = spawnTool('node', [
            '--test',
            'test/wasm-instance.test.mjs',
            'test/webgpu-compat.test.mjs',
            'test/offscreen-coalesce.test.mjs',
            'test/webgl-fallback.test.mjs',
        ]);
        if (postOptTests.status !== 0) {
            throw new Error('Post-wasm-opt initialization tests failed');
        }
    }
}

const acquireBuildLock = async () => {
    const lockPath = `${getLibDirectory()}/.build.lock`;
    for (;;) {
        try {
            const fd = openSync(lockPath, 'wx');
            writeFileSync(fd, String(process.pid));
            closeSync(fd);
            return () => rmSync(lockPath, {force: true});
        } catch {
            try {
                const holder = Number(readFileSync(lockPath, 'utf8').trim());
                if (holder > 0) process.kill(holder, 0);
            } catch {
                rmSync(lockPath, {force: true});
                continue;
            }
            await new Promise(resolve => setTimeout(resolve, 250));
        }
    }
}

const esbuild = async (name, globalName = undefined) => {
    let result = await build({...config, format: name, globalName, outfile: `dist/esbuild-${name}/module.js`,});
    console.log(result.errors.length === 0 ? "No errors." : "Found errors.")
}

const start = async () => {
    const releaseLock = await acquireBuildLock();
    try {
        console.log("Creating WASM...")
        wasmPack();

        if (esm) {
            console.log("Building esm bundle...")
            await esbuild("esm")
        }

        if (cjs) {
            console.log("Building cjs bundle...")
            await esbuild("cjs")
        }

        if (iife) {
            console.log("Building iife bundle...")
            await esbuild("iife", "maplibre")
        }

        console.log("Emitting TypeScript types...")
        emitTypeScript();
    } catch (e) {
        console.error("Failed to start building: " + e.message)
        process.exitCode = 1;
    } finally {
        releaseLock();
    }
}

const _ = start()
