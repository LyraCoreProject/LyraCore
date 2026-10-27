#!/usr/bin/env python3
"""Select the taxi scenario's rows from LyraCore importer dry-run SQL; see README.md."""

import argparse
from pathlib import Path
import re


# Importer strings use doubled SQL quotes; parentheses and commas can occur inside them.
ROW = re.compile(r"\((?:'(?:[^']|'')*'|[^'()])*\)")
TABLES = {
    'game_taxi_node': (0, {2, 4}, 2),
    'game_taxi_path': (0, {6}, 1),
    'game_taxi_path_node': (1, {6}, 28),
    'game_creature_template': (0, {352}, 1),
}


def select_rows(paths):
    selected = {table: [] for table in TABLES}
    columns = {}
    for path in paths:
        with Path(path).open() as source:
            for line in source:
                match = re.fullmatch(r'INSERT INTO (\w+) \(([^)]+)\) VALUES (.*);\s*', line)
                if not match or match[1] not in TABLES:
                    continue
                table, fields, values = match.groups()
                if table in columns and columns[table] != fields:
                    raise ValueError(f'{table}: inconsistent columns')
                columns[table] = fields
                index, wanted, _ = TABLES[table]
                remaining = values
                while remaining:
                    row = ROW.match(remaining)
                    if row is None:
                        raise ValueError(f'{table}: malformed importer row')
                    text = row[0]
                    key = int(text[1:-1].split(',', index + 1)[index])
                    if key in wanted:
                        selected[table].append(text)
                    remaining = remaining[row.end():]
                    if remaining:
                        if not remaining.startswith(','):
                            raise ValueError(f'{table}: malformed row separator')
                        remaining = remaining[1:]
    statements = []
    for table, (_, _, count) in TABLES.items():
        rows = selected[table]
        if len(rows) != count:
            raise ValueError(f'{table}: expected {count} imported rows, got {len(rows)}')
        statements.append(f"INSERT INTO {table} ({columns[table]}) VALUES {','.join(rows)};")
    return '\n'.join(statements) + '\n'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('sql', nargs='+', type=Path, help='LyraCore importer dry-run SQL, not a source dump')
    args = parser.parse_args()
    print(select_rows(args.sql), end='')


if __name__ == '__main__':
    main()
