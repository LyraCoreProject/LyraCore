"""Materialize migration defaults for older SQL INSERT statements."""

import re
from pathlib import Path

DEFAULTS = {}
for declaration in (Path(__file__).resolve().parents[1] / 'crates/lyracore-shared/src/import_sql_defaults.txt').read_text().splitlines():
    table, column, value = declaration.split()
    DEFAULTS.setdefault(table, []).append((column, value))


def canonical(name):
    """SpacetimeDB 2.14 publishes `convert_case` 0.6 snake_case column names (`value1` as `value_1`)."""
    words = ['']
    for at, char in enumerate(name):
        if char in '_- ':
            words.append('')
            continue
        before = name[at - 1] if at else ''
        after = name[at + 1:at + 2]
        if (before.islower() and char.isupper()) or (before.isupper() and char.isdigit()) \
                or (before.isdigit() and char.isalpha()) or (before.islower() and char.isdigit()) \
                or (before.isupper() and char.isupper() and after.islower()):
            words.append('')
        words[-1] += char.lower()
    return '_'.join(word for word in words if word)


def statements_of(query):
    return re.split(r"(;)(?=(?:[^']*'[^']*')*[^']*$)", query)


def insert_defaults(query):
    statements = statements_of(query)
    for index, statement in enumerate(statements):
        match = re.match(r'\s*INSERT INTO (\w+)\s*\(([^)]*)\)\s*VALUES', statement)
        if not match or match[1] not in DEFAULTS:
            continue
        columns = [column.strip() for column in match[2].split(',')]
        named = {canonical(column) for column in columns}
        missing = [(name, value) for name, value in DEFAULTS[match[1]] if canonical(name) not in named]
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
    """SpacetimeDB SQL assigns INSERT values by table position, so write named columns in that order.

    Raises ValueError for a column the table lacks, a column named twice, unreadable VALUES or a row of
    the wrong length.
    """
    statements = statements_of(query)
    for index, statement in enumerate(statements):
        match = re.match(r'(\s*INSERT INTO (\w+)\s*)\(([^)]*)\)(\s*VALUES\s*)(.*)$', statement, re.S)
        if not match:
            continue
        table = match[2]
        columns = [column.strip() for column in match[3].split(',')]
        named = [canonical(column) for column in columns]
        order = columns_of(table)
        for at, column in enumerate(columns):
            if named[at] not in order:
                raise ValueError(f'{table} has no column `{column}`')
            if named[at] in named[:at]:
                raise ValueError(f'{table}.{column} is named twice')
        rows = value_rows(match[5])
        if rows is None:
            raise ValueError(f'{table}: malformed VALUES')
        for row in rows:
            if len(row) != len(columns):
                raise ValueError(f'{table}: {len(row)} values for {len(columns)} columns')
        ordered = [column for column in order if column in named]
        positions = [named.index(column) for column in ordered]
        values = ','.join('(' + ','.join(row[position] for position in positions) + ')' for row in rows)
        statements[index] = f"{match[1]}({','.join(ordered)}){match[4]}{values}"
    return ''.join(statements)
