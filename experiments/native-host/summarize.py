#!/usr/bin/env python3
import hashlib
import json
from pathlib import Path
import re
import tarfile

ROOT = Path(__file__).resolve().parent
DESTINATION = ROOT / 'results'


def metrics(path):
    values = {}
    if not path.exists():
        return values
    for line in path.read_text().splitlines():
        if line and not line.startswith('#'):
            name, value = line.rsplit(' ', 1)
            if name.startswith(('view_calls{', 'view_total_time_usec{',
                                'spacetime_num_delta_queries_evaluated{',
                                'spacetime_reducer_plus_query_duration_sec_sum{',
                                'spacetime_txn_cpu_time_sec_sum{')):
                values[name] = float(value)
    return values


def main():
    DESTINATION.mkdir(exist_ok=True)
    runs = []
    files = []
    for path in sorted((ROOT / 'runs').glob('*/result.json')):
        record = json.loads(path.read_text())
        if 'observed' not in record and not ('startup' in record and 'error' in record):
            continue
        record['run_id'] = path.parent.name
        record.pop('run', None)
        record.pop('host_pid', None)
        record.get('observed', {}).pop('histogram_ms', None)
        record.get('observed', {}).pop('entities', None)
        record['completed_run'] = record.get('success', False)
        record['met_100ms_budget'] = bool(record.get('all_updates_received') and
            record.get('completed_ticks', record['seconds_requested'] * 10) == record['seconds_requested'] * 10 and
            record['observed'].get('p99_ms', 100_000) < 100)
        before, after = metrics(path.parent / 'metrics-before.txt'), metrics(path.parent / 'metrics-after.txt')
        deltas = {}
        for key, value in after.items():
            delta = value - before.get(key, 0)
            if delta:
                label = re.sub(r'(db|database_identity)="[0-9a-f]+",?', '', key)
                deltas[label] = delta
        record['metric_deltas'] = deltas
        record['host_queue_overflow'] = False
        record['native_queue_overflow'] = False
        log_path = path.parent / 'host.log'
        if log_path.exists():
            log = log_path.read_text()
            record['host_queue_overflow'] = 'exceeded channel capacity' in log
            record['native_queue_overflow'] = 'native delivery queue full' in log
        runs.append(record)
        for name in ['result.json', 'metrics-before.txt', 'metrics-after.txt', 'batches.json', 'driver.log']:
            file = path.parent / name
            if file.exists():
                files.append(file)
        if log_path.exists():
            warnings = '\n'.join(line for line in log_path.read_text().splitlines() if ' WARN ' in line or ' ERROR ' in line)
            warning_path = path.parent / 'warnings.txt'
            warning_path.write_text(warnings + '\n')
            files.append(warning_path)
    source_files = [p for p in ROOT.rglob('*') if p.is_file() and
        not any(part in {'target', 'runs', 'results', '__pycache__'} for part in p.relative_to(ROOT).parts)]
    hashes = {str(p.relative_to(ROOT)):hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(source_files)}
    summary = {'date':'2026-10-23', 'host_revision':'08e1aba133671f9c631c6e70133a82faa0e7b283',
        'host_version':'2.13.0', 'final_implementation_revision':3,
        'hardware':'Intel Core i7-7820HQ, 4 cores / 8 threads, 15 GiB RAM; host and driver share the machine',
        'source_sha256':hashes, 'runs':runs}
    (DESTINATION / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
    with tarfile.open(DESTINATION / 'raw.tar.gz', 'w:gz') as archive:
        for path in files:
            archive.add(path, arcname=str(path.relative_to(ROOT / 'runs')))
    print(f'{len(runs)} runs; {len(files)} raw evidence files')


if __name__ == '__main__':
    main()
