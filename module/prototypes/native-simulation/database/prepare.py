#!/usr/bin/env python3
import argparse
import hashlib
import shutil
import subprocess
from pathlib import Path


def replace_once(source, old, new):
    if source.count(old) != 1:
        raise ValueError(f"pinned SDK source did not match: {old[:60]}")
    return source.replace(old, new, 1)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("server", type=Path)
    parser.add_argument("sdk_source", type=Path)
    parser.add_argument("sdk_copy", type=Path)
    args = parser.parse_args()
    here = Path(__file__).resolve().parent
    revision = subprocess.check_output(
        ["git", "-C", str(args.server), "rev-parse", "HEAD"], text=True
    ).strip()
    if revision != "8ecd3c3bac427a49a9d9b13671bf83324afa59ba":
        raise ValueError("server must be the pinned SpacetimeDB v2.7.1 source")
    source = (args.sdk_source / "src/rt.rs").read_bytes()
    if hashlib.sha256(source).hexdigest() != "9dae2fdd25e809c3fae6780569a1e6ce1a9d76d99feaac6a38a884faaa5ebf16":
        raise ValueError("SDK must be the unmodified crates.io 2.7.1 source")
    patch = here / "spacetimedb-2.7.1.patch"
    subprocess.run(["git", "-C", str(args.server), "apply", "--check", str(patch)], check=True)
    if args.sdk_copy.exists():
        raise ValueError("SDK copy already exists")
    shutil.copytree(args.sdk_source, args.sdk_copy)
    source = source.decode()
    source = replace_once(source, 'extern "C" fn __describe_module__(description: BytesSink) {', '''extern "C" fn __describe_module__(description: BytesSink) {
    describe_module_inner(description)
}

fn describe_module_inner(description: BytesSink) {''')
    start = source.index('extern "C" fn __call_reducer__(')
    body = source.index(') -> i16 {', start) + len(') -> i16 {')
    signature = source[start:body]
    source = source[:start] + signature + '''
    call_reducer_inner(id, sender_0, sender_1, sender_2, sender_3, conn_id_0, conn_id_1, timestamp, args, error)
}

''' + signature.replace('extern "C" fn __call_reducer__', 'fn call_reducer_inner') + source[body:]
    source += '\n#[cfg(not(target_arch = "wasm32"))]\n#[path = "native_bridge.rs"]\nmod native_bridge;\n'
    (args.sdk_copy / "src/rt.rs").write_text(source)
    manifest = args.sdk_copy / "Cargo.toml"
    with manifest.open("a") as output:
        output.write('\n[target.\'cfg(not(target_arch = "wasm32"))\'.dependencies.libm]\n'
                     'version = "=0.2.16"\n')
    shutil.copyfile(here / "sdk_bridge.rs", args.sdk_copy / "src/native_bridge.rs")
    shutil.copyfile(here / "native_math.rs", args.sdk_copy / "src/native_math.rs")
    shutil.copyfile(here / "syscalls.rs", args.sdk_copy / "src/native_syscalls.rs")
    subprocess.run(["git", "-C", str(args.server), "apply", str(patch)], check=True)
    shutil.copyfile(here / "host.rs", args.server / "crates/core/src/host/wasmtime/native_database.rs")


if __name__ == "__main__":
    main()
