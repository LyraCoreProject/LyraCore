const fs = require("node:fs");

async function main() {
    const [wasm, baseline, native, count] = process.argv.slice(2);
    const { instance } = await WebAssembly.instantiate(fs.readFileSync(wasm));
    const versions = { baseline: fs.readFileSync(baseline), native: fs.readFileSync(native) };
    const operations = ["atan", "atan2", "atan2f", "cos", "cosf", "hypotf", "log", "pow",
        "sin", "sinf", "tan", "sincos_sin", "sincos_cos", "sincosf_sin", "sincosf_cos"];
    const samples = Number(count);
    const mismatches = {};
    for (const [name, bytes] of Object.entries(versions)) {
        if (bytes.length !== samples * operations.length * 8) throw Error("wrong sample length");
        mismatches[name] = Object.fromEntries(operations.map(name => [name, 0]));
    }
    let seed = 123456789;
    for (let sample = 0; sample < samples; sample++) {
        seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0;
        const pointer = instance.exports.evaluate(seed);
        const expected = new DataView(instance.exports.memory.buffer, pointer, operations.length * 8);
        for (let operation = 0; operation < operations.length; operation++) {
            const value = expected.getBigUint64(operation * 8, true);
            const offset = (sample * operations.length + operation) * 8;
            for (const [name, bytes] of Object.entries(versions)) {
                if (value !== bytes.readBigUInt64LE(offset)) mismatches[name][operations[operation]]++;
            }
        }
    }
    process.stdout.write(JSON.stringify({ samples, mismatches }) + "\n");
    if (Object.values(mismatches.native).some(count => count !== 0)) process.exitCode = 1;
}

main().catch(error => { console.error(error); process.exitCode = 1; });
