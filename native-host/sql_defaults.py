"""Materialize the defaults omitted by spell SQL fixtures predating the additive schema."""

import re

DEFAULTS = {
    'game_spell': [('custom_flags', '0'), ('min_target_level', '0'), ('single_target_key', '0')],
    'game_spell_effect': [('bonus_coefficient', '-1.0')],
}


def spell_defaults(query):
    statements = re.split(r"(;)(?=(?:[^']*'[^']*')*[^']*$)", query)
    for index, statement in enumerate(statements):
        match = re.match(r'\s*INSERT INTO (game_spell|game_spell_effect)\s*\(([^)]*)\)\s*VALUES', statement)
        if not match:
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
