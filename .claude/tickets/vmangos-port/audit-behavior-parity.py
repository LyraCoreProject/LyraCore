import collections,json,re,sys
from pathlib import Path


def catalogue(path, classic=False):
    credit_quests=collections.defaultdict(set)
    if not classic:
        for statement in json.loads(Path(sys.argv[4]).read_text()):
            if statement.startswith('INSERT INTO game_quest_objective '):
                for _,quest,_,kind,target,_ in re.findall(r'\((\d+),(\d+),(\d+),(\d+),(\d+),(\d+)\)',statement):
                    if kind=='0':credit_quests[target].add(quest)
    batches=json.loads(Path(path).read_text())
    relays={}
    for reducer,batch in batches:
        if reducer.startswith('import_creature_ai_relay_definitions'):
            for line in batch.splitlines():
                fields=line.split('@',4)
                relays[int(fields[0])]=[step.split(',',4)[-1] for step in fields[4].split('~')]
    def expanded(instructions,seen=frozenset()):
        for instruction in instructions:
            fields=instruction.split(':')
            if fields[0] in ('start-relay','start-script'):
                for relay in map(int,fields[1].split('.')):
                    if relay not in seen:
                        yield from expanded(relays.get(relay,[]),seen|{relay})
            else:
                yield fields
    entries=collections.defaultdict(lambda:collections.defaultdict(set))
    for reducer,batch in batches:
        if not reducer.startswith('import_creature_ai_definitions'):continue
        for line in batch.splitlines():
            subject,revision,rules=line.split('@',2)
            kind,number=subject.split(':')
            entry=int(number) if kind=='entry' else (int(number)>>24)&0xffffff
            for rule in rules.split('~'):
                for fields in expanded(rule.split(',')[-1].split('+')):
                    op=fields[0]
                    if op=='cast':category,keys='Spells',fields[1:2]
                    elif op in ('speak','talk'):
                        category='Speech'
                        keys=fields[-1].split('.') if op=='speak' else fields[1].split('.')
                        if '1150' in keys:entries[entry]['Flee text'].add('1150')
                    elif op in ('summon','spawn-at-actor','spawn-creature'):category,keys='Summons',fields[1:2]
                    elif op in ('emote','random-emote'):category,keys='Emotes',fields[1].split('.')
                    elif op=='flee':
                        category,keys='Flee',['flee']
                        if classic:entries[entry]['Flee text'].add('1150')
                    elif op in ('quest-event','kill-credit'):
                        category='Quest credit'
                        keys=['quest:'+fields[1]] if op=='quest-event' else ['quest:'+q for q in credit_quests.get(fields[1],[])] or ['creature:'+fields[1]]
                    elif op=='notify-encounter':category,keys='Encounter notifications',[':'.join(fields[1:])]
                    elif op.startswith('phase'):category,keys='Phase control',[op+':'+':'.join(fields[1:])]
                    elif op in ('unit-flags','npc-flags','faction','display','display-template','sheath','set-stand-state','creature-mount','virtual-main-hand'):
                        category,keys='Flags and appearance',[op+':'+':'.join(fields[1:])]
                    elif op in ('combat-movement','follow-movement','ranged-mode','immobilized','walking','idle','patrol-paused','facing','posture','relocate-source','move-source'):
                        category,keys='Movement and posture',[op+':'+':'.join(fields[1:])]
                    else:category,keys='Other actions',[op+':'+':'.join(fields[1:])]
                    entries[entry][category].update(keys)
    return entries

classic,before,after=catalogue(sys.argv[1],True),catalogue(sys.argv[2]),catalogue(sys.argv[3])
def state(expected,actual):
    return 0 if expected<=actual else 1 if expected&actual else 2

def counts(candidate,category=None):
    totals=[0,0,0]
    for entry,behaviors in classic.items():
        expected={(kind,key) for kind,keys in behaviors.items() if category is None or category==kind for key in keys}
        if not expected:continue
        actual={(kind,key) for kind,keys in candidate.get(entry,{}).items() for key in keys}
        totals[state(expected,actual)]+=1
    return totals
print('# EventAI action coverage before and after the M4 fixes\n')
print('This compares the full-world emitted plans: classic-db, vmangos at `928e3489`, and the corrected vmangos plan. A creature is an entry; GUID rules are grouped with their creature entry, and reachable relay actions are expanded. No source rows are reproduced.\n')
print('“Equivalent” means every classic action key remains available; “reduced” means some remain; “lost” means none remain. Keys are spell, text, summon or emote ids, quest ids reached by event or creature-objective credit, and typed operands for other actions. This is static action coverage, not verified gameplay equivalence: it does not compare event timing, chance, conditions, targets, command flags or source-only extra actions. Flee text is a separate expected behavior (broadcast 1150). Native and durable test results are reported separately.\n')
print('| Behavior | Classic creatures | Before equivalent / reduced / lost | After equivalent / reduced / lost |\n| --- | ---: | --- | --- |')
for category in [None,*sorted({kind for behaviors in classic.values() for kind in behaviors})]:
    first,last=counts(before,category),counts(after,category)
    print(f'| {category or "All behaviors"} | {sum(first)} | {" / ".join(map(str,first))} | {" / ".join(map(str,last))} |')
