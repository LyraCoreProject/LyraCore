"""Materialize migration defaults for older SQL INSERT statements."""

import re
from pathlib import Path

DEFAULTS = {}
for declaration in (Path(__file__).resolve().parents[1] / 'crates/lyracore-shared/src/import_sql_defaults.txt').read_text().splitlines():
    table, column, value = declaration.split()
    DEFAULTS.setdefault(table, []).append((column, value))


def spell_defaults(query):
    statements = re.split(r"(;)(?=(?:[^']*'[^']*')*[^']*$)", query)
    for index, statement in enumerate(statements):
        match = re.match(r'\s*INSERT INTO (\w+)\s*\(([^)]*)\)\s*VALUES', statement)
        if not match or match[1] not in DEFAULTS:
            continue
        columns = [column.strip() for column in match[2].split(',')]
        missing = [(name, value) for name, value in DEFAULTS[match[1]] if name not in columns]
        if not missing:
            continue
        close = statement.index(')', match.start(2))
        output = [statement[:close], ''.join(',' + name for name, _ in missing), statement[close:match.end()]]
        depth, quoted = 0, False
        for char in statement[match.end():]:
            if char == "'":
                quoted = not quoted
            if not quoted:
                if char == '(':
                    depth += 1
                elif char == ')':
                    depth -= 1
                    if depth == 0:
                        output.append(''.join(',' + value for _, value in missing))
            output.append(char)
        statements[index] = ''.join(output)
    return ''.join(statements)
