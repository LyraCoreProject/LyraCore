#!/usr/bin/env python3
import argparse
import os
from pathlib import Path
import re
import shutil
import subprocess

REVISION = '08e1aba133671f9c631c6e70133a82faa0e7b283'
ROOT = Path(__file__).resolve().parent


def host_toolchain():
    pin = ROOT / 'rust-toolchain.toml'
    match = re.search(r'^\s*channel\s*=\s*"([^"]+)"', pin.read_text(), re.M)
    if not match:
        raise RuntimeError(f'cannot read the toolchain channel from {pin}')
    return match[1]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--checkout', type=Path, required=True)
    parser.add_argument('--build', action='store_true')
    args = parser.parse_args()
    checkout = args.checkout.resolve()
    if not checkout.exists():
        subprocess.run(['git', 'clone', '--depth', '1', '--branch', 'v2.13.0',
            'https://github.com/clockworklabs/SpacetimeDB.git', str(checkout)], check=True)
    revision = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=checkout, text=True).strip()
    if revision != REVISION:
        raise RuntimeError(f'expected SpacetimeDB {REVISION}, got {revision}')
    changes = {
        'crates/standalone/Cargo.toml': ('[dependencies]', '[dependencies]\nwow_srp = "=0.7.0"\nspacetimedb-auth.workspace = true'),
        'crates/standalone/src/lib.rs': ('mod control_db;', 'mod control_db;\npub mod native_world;'),
        'crates/standalone/src/subcommands/start.rs': ('    worker_metrics::spawn_jemalloc_stats',
            '    crate::native_world::start(ctx.clone()).await?;\n    worker_metrics::spawn_jemalloc_stats'),
    }
    for name, (old, new) in changes.items():
        original = subprocess.check_output(['git', 'show', f'HEAD:{name}'], cwd=checkout, text=True)
        if original.count(old) != 1:
            raise RuntimeError(f'unexpected upstream file: {name}')
        desired = original.replace(old, new, 1)
        target = checkout / name
        current = target.read_text()
        if current not in (original, desired):
            raise RuntimeError(f'refusing to overwrite unrelated changes in {target}')
        if current != desired:
            target.write_text(desired)
    for name in ('host-lock.patch', 'dev-reload.patch', 'compile-cache.patch', 'retention.patch',
            'independent-snapshots.patch'):
        patch = str(ROOT / name)
        check = subprocess.run(['git', 'apply', '--check', patch], cwd=checkout, capture_output=True)
        if check.returncode == 0:
            subprocess.run(['git', 'apply', patch], cwd=checkout, check=True)
        else:
            subprocess.run(['git', 'apply', '--reverse', '--check', patch], cwd=checkout, check=True)
    sources = {
        'retention': 'crates/engine/src/retention.rs',
        'compile_cache': 'crates/core/src/host/wasmtime/compile_cache.rs',
        'mod': 'crates/standalone/src/native_world.rs',
        'feed': 'crates/standalone/src/native_feed.rs',
        'wire': 'crates/standalone/src/native_wire.rs',
        'delivery_rows': 'crates/standalone/src/native_delivery_rows.rs',
    }
    for name, destination in sources.items():
        source, target = ROOT / f'src/{name}.rs', checkout / destination
        if not target.is_file() or source.read_bytes() != target.read_bytes():
            shutil.copyfile(source, target)
    if args.build:
        env = dict(os.environ, CARGO_BUILD_JOBS=os.environ.get('CARGO_BUILD_JOBS', '3'),
            CARGO_PROFILE_RELEASE_DEBUG='0', CARGO_PROFILE_RELEASE_LTO='false')
        subprocess.run(['cargo', f'+{host_toolchain()}', 'build', '--release', '--locked', '-p', 'spacetimedb-standalone'], cwd=checkout, env=env, check=True)
    print(checkout / 'target/release/spacetimedb-standalone')


if __name__ == '__main__':
    main()
