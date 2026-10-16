#!/usr/bin/env python3
import argparse
import os
from pathlib import Path


def positive(value):
    number = int(value)
    if number <= 0:
        raise argparse.ArgumentTypeError("must be positive")
    return number


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--cpu-percent", type=positive, required=True)
    parser.add_argument("--memory-mib", type=positive, required=True)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command
    if command[:1] == ["--"]:
        command = command[1:]
    if not command:
        parser.error("supply the standalone command after --")
    if not args.manifest.is_file():
        parser.error("the native manifest must be an existing file")
    environment = os.environ.copy()
    environment["LYRACORE_NATIVE_DATABASE_MANIFEST"] = str(args.manifest.resolve(strict=True))
    os.execvpe("systemd-run", [
        "systemd-run", "--user", "--scope", "--quiet",
        f"--property=CPUQuota={args.cpu_percent}%",
        f"--property=MemoryMax={args.memory_mib}M",
        "--property=MemorySwapMax=0", "--property=OOMPolicy=kill",
        "--", *command,
    ], environment)


if __name__ == "__main__":
    main()
