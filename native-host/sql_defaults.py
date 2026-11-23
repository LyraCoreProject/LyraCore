"""Materialize migration defaults for older SQL INSERT statements."""

import re
from pathlib import Path

DEFAULTS = {}
for declaration in (Path(__file__).resolve().parents[1] / 'crates/lyracore-shared/src/import_sql_defaults.txt').read_text().splitlines():
    table, column, value = declaration.split()
    DEFAULTS.setdefault(table, []).append((column, value))


def statements_of(query):
    return re.split(r"(;)(?=(?:[^']*'[^']*')*[^']*$)", query)


def insert_defaults(query):
    statements = statements_of(query)
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


def value_rows(text):
    """Splits `(a,b),(c,d)` into value lists, or returns None when the text is not only tuples."""
    rows, row, cell, depth, quoted = [], [], '', 0, False
    for char in text.strip():
        if char == "'":
            quoted = not quoted
        if not quoted and char == '(':
            depth += 1
            if depth == 1:
                continue
        if not quoted and char == ')':
            depth -= 1
            if depth == 0:
                rows.append(row + [cell.strip()])
                row, cell = [], ''
                continue
        if depth == 0:
            if char not in ', \n\t':
                return None
        elif not quoted and depth == 1 and char == ',':
            row.append(cell.strip())
            cell = ''
        else:
            cell += char
    return rows if depth == 0 and not quoted else None


def in_table_order(query, columns_of):
    """SpacetimeDB SQL assigns INSERT values by table position, so put named columns in that order."""
    statements = statements_of(query)
    for index, statement in enumerate(statements):
        match = re.match(r'(\s*INSERT INTO (\w+)\s*)\(([^)]*)\)(\s*VALUES\s*)(.*)$', statement, re.S)
        if not match:
            continue
        columns = [column.strip() for column in match[3].split(',')]
        order = columns_of(match[2])
        ordered = [column for column in order if column in columns]
        rows = value_rows(match[5])
        if ordered == columns or len(ordered) != len(columns) or rows is None \
                or any(len(row) != len(columns) for row in rows):
            continue
        positions = [columns.index(column) for column in ordered]
        values = ','.join('(' + ','.join(row[position] for position in positions) + ')' for row in rows)
        statements[index] = f"{match[1]}({','.join(ordered)}){match[4]}{values}"
    return ''.join(statements)
