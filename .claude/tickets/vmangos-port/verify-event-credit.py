from pathlib import Path
import json, re, sys, struct
sys.path.insert(0, str(Path(__file__).resolve().parents[3] / 'native-host'))
from host import Host
from check import spawn

def selected(statement, column, ids):
    prefix, values = statement.split(' VALUES ',1)
    rows, start, depth, quoted, index = [], 0, 0, False, 0
    while index < len(values):
        char=values[index]
        if char == "'":
            if quoted and index+1<len(values) and values[index+1]=="'":
                index+=2
                continue
            quoted=not quoted
        elif not quoted:
            if char=='(':
                if depth==0:start=index
                depth+=1
            elif char==')':
                depth-=1
                if depth==0:
                    row=values[start:index+1]
                    if int(row[1:-1].split(',')[column]) in ids:rows.append(row)
        index+=1
    return prefix+' VALUES '+','.join(rows) if rows else None

import argparse
parser=argparse.ArgumentParser(description='Verify the event-credit fallback with locally emitted vmangos plans.')
parser.add_argument('--host',required=True)
parser.add_argument('--module',type=Path,required=True)
parser.add_argument('--plans',type=Path,required=True)
args=parser.parse_args()
plans=args.plans

def load(host, family, tables, ids):
    for statement in json.loads((plans/f'vmangos-full-world-{family}-statements.json').read_text()):
        if not statement.startswith('INSERT INTO '): continue
        table=statement.split(' ',3)[2]
        if table not in tables: continue
        columns=statement.split('(',1)[1].split(')',1)[0].split(',')
        column=columns.index(tables[table])
        if column == 0 and not any(f'({entry},' in statement for entry in ids): continue
        query=selected(statement,column,ids)
        if query: host.sql(query)

with Host(args.host,args.module,seeded=True) as host:
    load(host,'quests',{'game_quest_template':'entry','game_quest_objective':'quest_entry','game_creature_quest':'quest_entry','game_quest_reward_spell':'quest_entry','game_quest_event_requirement':'quest_entry'},{6064,6084,6085,6086})
    load(host,'creatures',{'game_creature_template':'entry','game_creature_level_stats':'entry','game_creature_display':'entry','game_creature_equipment':'creature_entry'},{1231,1196})
    load(host,'items',{'game_item_template':'entry'},{15908,15911,15913})
    load(host,'spell',{'game_spell':'spell_id','game_spell_effect':'spell_id'},{19548,19597,23356,1579,1515,883,2641})
    client=host.enter_world('Taming',race=3,cls=3,at=(0,8000,8000,0))
    guid=client.guid
    host.call('debug_set_level',[guid,10])
    for quest in [6064,6084]:
        host.call('debug_grant_quest',[guid,quest])
        host.sql(f'UPDATE game_character_quest SET rewarded = true WHERE character_guid = {guid} AND quest_entry = {quest}')
    giver=spawn(host,guid,1231,2)
    prey=spawn(host,guid,1196,5)
    client.collect()
    client.send(0x189,struct.pack('<QI',giver,6085))
    accepted=client.collect()
    assert host.rows(f'SELECT rewarded FROM game_character_quest WHERE character_guid = {guid} AND quest_entry = 6085')==[[False]],accepted
    assert not host.rows('SELECT id FROM game_quest_event_requirement WHERE quest_entry = 6085')
    print('PASS actual quest acceptance: dwarf hunter receives Taming the Beast without an unsupported event gate',flush=True)
    assert host.rows(f'SELECT slot FROM game_item_instance WHERE owner_guid = {guid} AND entry = 15908')
    print('PASS hunter retains the required Taming Rod for normal-objective turn-in',flush=True)
    client.send(0x18a,struct.pack('<QI',giver,6085))
    offer=client.collect()
    assert any(opcode==0x18d for opcode,_ in offer),offer
    client.send(0x18e,struct.pack('<QII',giver,6085,0))
    rewarded=client.collect()
    assert any(opcode==0x191 and body[:4]==struct.pack('<I',6085) for opcode,body in rewarded),rewarded
    assert host.rows(f'SELECT rewarded FROM game_character_quest WHERE character_guid = {guid} AND quest_entry = 6085')==[[True]]
    client.send(0x189,struct.pack('<QI',giver,6086))
    training=client.collect()
    assert host.rows(f'SELECT rewarded FROM game_character_quest WHERE character_guid = {guid} AND quest_entry = 6086')==[[False]],training
    print('PASS actual reward and quest chain: Taming the Beast completed; Training the Beast accepted',flush=True)
    print('Tame Beast learned',host.rows(f'SELECT spell_id FROM game_player_spell WHERE character_guid = {guid} AND spell_id = 1515'),flush=True)
