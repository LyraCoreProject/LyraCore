#!/usr/bin/env python3
import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import tempfile
import time
from pathlib import Path


def native_evidence(log):
    reducers = sorted(set(re.findall(r"LYRACORE_NATIVE_DATABASE_V1 reducer=(\w+)", log)))
    if "LYRACORE_NATIVE_DATABASE_FALLBACK" in log:
        raise ValueError("native execution fell back to Wasm")
    if "LYRACORE_NATIVE_DATABASE_WASM" in log:
        raise ValueError("fixture published a program without a matching native artifact")
    if not reducers:
        raise ValueError("fixture has no native reducer evidence")
    return reducers


def test_counts(log):
    results = re.findall(r"test result: (ok|FAILED)\. (\d+) passed; (\d+) failed;", log)
    if not results or any(status != "ok" or int(failed) for status, _, failed in results):
        raise ValueError("the command did not report a passing Rust test result")
    passed = sum(int(passed) for _, passed, _ in results)
    if not passed:
        raise ValueError("the command ran no tests")
    return passed


def standalone_logs(directory):
    # Standalone fixture names end in their process ID and timestamp.
    return sorted(path for path in directory.glob("*.log")
                  if re.search(r"-\d+-\d+\.log$", path.name))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("implementation", choices=("module", "native_database"))
    parser.add_argument("manifest", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command
    if command[:1] == ["--"]:
        command = command[1:]
    if not command:
        parser.error("supply a Rust test command after --")
    manifest = json.loads(args.manifest.read_text())
    environment = os.environ.copy()
    environment.pop("LYRACORE_NATIVE_DATABASE_MANIFEST", None)
    if args.implementation == "native_database":
        environment["LYRACORE_NATIVE_DATABASE_MANIFEST"] = str(args.manifest.resolve())
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
    dirty = bool(subprocess.check_output(["git", "status", "--porcelain"]))
    server = Path(environment["LYRACORE_NATIVE_DATABASE_SERVER"])
    with server.open("rb") as source:
        server_hash = hashlib.file_digest(source, "sha256").hexdigest()
    with args.output.open("x") as output:
        with args.output.with_suffix(".command.log").open("x+") as command_log:
            scratch = Path(tempfile.mkdtemp(prefix="native-durable-"))
            environment["TMPDIR"] = str(scratch)
            started = time.monotonic()
            result = subprocess.run(command, env=environment, stdout=command_log,
                                    stderr=subprocess.STDOUT)
            elapsed = time.monotonic() - started
            command_log.seek(0)
            test_log = command_log.read()
            errors = []
            passed = 0
            try:
                passed = test_counts(test_log)
            except ValueError as error:
                errors.append(str(error))
            fixtures = []
            for path in standalone_logs(scratch / "lyracore-standalone-logs"):
                log = path.read_text(errors="replace")
                reducers = []
                if args.implementation == "native_database":
                    try:
                        reducers = native_evidence(log)
                    except ValueError as error:
                        errors.append(f"{path.name}: {error}")
                elif "LYRACORE_NATIVE_DATABASE_V1" in log:
                    errors.append(f"{path.name}: reference used native execution")
                fixtures.append({"fixture": path.name, "native_reducers": reducers})
            if not fixtures:
                errors.append("no disposable SpacetimeDB fixture logs were found")
            if result.returncode:
                errors.append(f"test command exited with status {result.returncode}")
            json.dump({
                "commit": revision, "dirty": dirty,
                "implementation": args.implementation,
                "passed": passed, "fixtures": fixtures, "errors": errors,
                "tests": re.findall(r"^test (.+) \.\.\. ok$", test_log, re.MULTILINE),
                "elapsed_seconds": elapsed,
                "server_sha256": server_hash,
                "wasm_blake3": manifest["wasm_blake3"],
                "module_native_blake3": manifest["native_blake3"],
            }, output, indent=2)
            output.write("\n")
    if errors:
        print(f"Failed fixture logs retained at {scratch}")
        raise SystemExit("\n".join(errors))
    shutil.rmtree(scratch)
    print(f"{args.implementation}: {passed} tests passed across {len(fixtures)} fixtures")


if __name__ == "__main__":
    main()
