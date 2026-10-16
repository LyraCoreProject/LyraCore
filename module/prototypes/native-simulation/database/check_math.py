#!/usr/bin/env python3
import argparse
import ctypes
import hashlib
import json
import shutil
import struct
import subprocess
from pathlib import Path


def samples(library, output, count):
    module = ctypes.CDLL(str(library.resolve()))
    module.evaluate.argtypes = [ctypes.c_uint32]
    module.evaluate.restype = ctypes.POINTER(ctypes.c_uint64)
    seed = 123456789
    with output.open("xb") as destination:
        for _ in range(count):
            seed = (seed * 1664525 + 1013904223) & 0xffffffff
            destination.write(struct.pack("<15Q", *module.evaluate(seed)[:15]))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("scratch", type=Path)
    parser.add_argument("--samples", type=int, default=1_000_000)
    parser.add_argument("--toolchain", default="1.93.0")
    args = parser.parse_args()
    if not 1 <= args.samples <= 1_000_000:
        parser.error("samples must be in 1..=1000000")
    args.scratch.mkdir()
    here = Path(__file__).resolve().parent
    for name in ["math_check.rs", "native_math.rs"]:
        shutil.copyfile(here / name, args.scratch / name)
    (args.scratch / "Cargo.toml").write_text('''[package]
name = "native-math-check"
version = "0.1.0"
edition = "2021"
[lib]
path = "math_check.rs"
crate-type = ["cdylib"]
[features]
native = []
[dependencies]
libm = "=0.2.16"
[profile.release]
codegen-units = 1
[workspace]
''')
    command = ["cargo", f"+{args.toolchain}", "build", "--offline", "--release",
               "--jobs=1", "--target-dir=target"]
    subprocess.run(command + ["--target=wasm32-unknown-unknown"], cwd=args.scratch, check=True)
    wasm = args.scratch / "target/wasm32-unknown-unknown/release/native_math_check.wasm"
    for name, features in [("baseline", []), ("native", ["--features=native"])]:
        subprocess.run(command + ["--locked"] + features, cwd=args.scratch, check=True)
        library = args.scratch / f"{name}.so"
        shutil.copyfile(args.scratch / "target/release/libnative_math_check.so", library)
        samples(library, args.scratch / f"{name}.bin", args.samples)
    comparison = subprocess.run(
        ["node", str(here / "math_check.js"), str(wasm),
         str(args.scratch / "baseline.bin"), str(args.scratch / "native.bin"), str(args.samples)],
        capture_output=True, text=True)
    if comparison.stderr:
        print(comparison.stderr, end="")
    result = json.loads(comparison.stdout)
    result["rustc"] = subprocess.check_output(["rustc", f"+{args.toolchain}", "-Vv"], text=True)
    result["sha256"] = {
        name: hashlib.sha256((args.scratch / name).read_bytes()).hexdigest()
        for name in ["math_check.rs", "native_math.rs", "baseline.so", "native.so"]
    }
    result["sha256"]["wasm"] = hashlib.sha256(wasm.read_bytes()).hexdigest()
    (args.scratch / "comparison.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result))
    comparison.check_returncode()


if __name__ == "__main__":
    main()
