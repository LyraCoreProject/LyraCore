#!/usr/bin/env python3
"""Fail when a Module wasm imports from `env`, which means it linked unresolved native code."""

import argparse
import sys
from pathlib import Path

IMPORT_SECTION = 2


def leb128(data, offset):
    value = shift = 0
    while True:
        byte = data[offset]
        offset += 1
        value |= (byte & 0x7F) << shift
        shift += 7
        if byte < 0x80:
            return value, offset


def name(data, offset):
    length, offset = leb128(data, offset)
    return data[offset : offset + length].decode(), offset + length


def skip_import_kind(data, offset):
    kind = data[offset]
    offset += 1
    if kind == 0:
        return leb128(data, offset)[1]
    if kind == 1:
        offset += 1
        kind = 2
    if kind == 2:
        flags = data[offset]
        offset = leb128(data, offset + 1)[1]
        return leb128(data, offset)[1] if flags & 1 else offset
    if kind == 3:
        return offset + 2
    if kind == 4:
        return leb128(data, offset + 1)[1]
    raise ValueError(f"unknown import kind {kind:#x}")


def imports(data):
    if data[:4] != b"\0asm":
        raise ValueError("not a wasm module")
    offset = 8
    while offset < len(data):
        section = data[offset]
        size, offset = leb128(data, offset + 1)
        end = offset + size
        if section == IMPORT_SECTION:
            count, offset = leb128(data, offset)
            for _ in range(count):
                module, offset = name(data, offset)
                field, offset = name(data, offset)
                offset = skip_import_kind(data, offset)
                yield module, field
        offset = end


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("wasm", type=Path)
    args = parser.parse_args()
    env = sorted(field for module, field in imports(args.wasm.read_bytes()) if module == "env")
    if env:
        print(f"{args.wasm} imports from env: {', '.join(env)}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
