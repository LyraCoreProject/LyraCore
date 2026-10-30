def check_operator_spells(host):
    character = host.enter_world('Spelleditor', cls=8, at=(0, 20, 0, 0))
    host.call('debug_learn_spell', [character.guid, 133])

    def rows(table):
        result = host.sql(f'SELECT * FROM {table} WHERE spell_id = 133')[0]
        names = [column['name']['some'] for column in result['schema']['elements']]
        return [dict(zip(names, row)) for row in result['rows']]

    for table in ['game_spell', 'game_spell_effect', 'game_spell_reagent',
                  'game_trainer_spell', 'game_creature_ai_spell_metadata', 'game_operator_spell_source']:
        host.restore(table, 'spell_id = 133')
    spell = rows('game_spell')[0]
    spell['name'] = 'Operator bolt'
    spell['cost'] = 7
    known = host.rows(f'SELECT * FROM game_player_spell WHERE character_guid = {character.guid}')
    assert known
    edit = dict(create=False, spell=spell, effects=rows('game_spell_effect'),
                reagents=rows('game_spell_reagent'), trainers=rows('game_trainer_spell'),
                metadata=rows('game_creature_ai_spell_metadata'),
                source=[dict(name='entry', value='133'), dict(name='build', value='5875'),
                        dict(name='name', value='Operator bolt')])
    host.call('operator_apply_spell_edits', [[edit]])
    assert host.rows('SELECT cost FROM game_spell WHERE spell_id = 133') == [[7]]
    assert host.rows(f'SELECT * FROM game_player_spell WHERE character_guid = {character.guid}') == known
    edit['spell']['cost'] = 9
    edit['effects'][0]['id'] = 0
    try:
        host.call('operator_apply_spell_edits', [[edit]])
    except RuntimeError as refusal:
        assert 'effect key' in str(refusal), refusal
    else:
        raise AssertionError('an invalid spell effect succeeded')
    assert host.rows('SELECT cost FROM game_spell WHERE spell_id = 133') == [[7]]
