const fs = require('fs');
const crypto = require('crypto');
const path = require('path');
const _sodium = require('libsodium-wrappers');

// Mock a minimal browser environment that Go's wasm_exec.js might expect
global.window = global;
global.document = {
    readyState: 'complete',
    documentElement: {},
};
global.crypto = {
    getRandomValues(b) {
        crypto.randomFillSync(b);
    },
};
global.performance = {
    now() {
        return Date.now();
    }
};

// Load the downloaded Go WebAssembly runtime
require('./wasm_exec.js');

async function test_getAdv() {
    await _sodium.ready;
    global.sodium = _sodium;

    const go = new Go();

    // Path to the EXACT intercept output relative to this script
    const wasmPath = path.join(__dirname, 'fu.wasm');
    const wasmBuffer = fs.readFileSync(wasmPath);

    const result = await WebAssembly.instantiate(wasmBuffer, go.importObject);
    const instance = result.instance;

    // Start the Go runtime loop (this binds functions like 'getAdv' to the global object)
    go.run(instance);

    // Give it a tiny bit of time to attach to `global`
    await new Promise(r => setTimeout(r, 50));

    // Get media ID from command line, fallback to 157336
    const mediaId = process.argv[2] || "157336";

    if (typeof global.getAdv === 'function') {
        const tStart = performance.now();

        // Pass the media ID as the string arguments.
        const token = global.getAdv(mediaId);

        const tEnd = performance.now();
        // Since we are piping this to rust, we should only print the naked token
        console.log(token);
    } else {
        console.error("Failed to load getAdv");
        process.exit(1);
    }
}

test_getAdv();
