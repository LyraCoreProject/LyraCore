# Distract client check

Execution status: outstanding. The automated importer, module and gateway tests verify the
Distraction rules, the ground destination Gate and the `SMSG_SPELL_GO` shape. They do not verify
what the unmodified build-5875 client draws, or how a real Creature reacts to a visible or a
stealthed player.

Use only an isolated local LyraCore stack. Never point these steps at a production realm. Before you
log in, check the client's `realmlist.wtf` and `WTF/Config.wtf` and point both at 127.0.0.1. Restore
them afterward. Import the operator-owned 1.12.1 client data so spell 1725 exists
(`--dbc <Data dir> --spells --only 1725,1728`, plus the trainer binding the realm uses). Use a Rogue
of level 22 or higher with at least 30 energy who knows Distract. Record the server commit, client
build, character, and test position.

- [ ] Select Distract. Confirm the cursor becomes a ground reticle, a point 30 yards away is
      accepted, and a point beyond it gives the out-of-range message.
- [ ] Cast it at a point 5 yards from an idle Creature. Confirm energy drops by 30, the button shows
      a 30 second cooldown, and the Rogue stays stealthed if stealthed before the cast.
- [ ] Confirm the Creature turns toward the point. Record whether it snaps or turns smoothly, and
      whether it turns at once.
- [ ] Confirm it holds that facing for 10 seconds, then turns back to its original facing. Record the
      real time with a stopwatch.
- [ ] Confirm where the cast animation plays, at the destination or at the caster. Confirm no impact
      flash shows on the Rogue.
- [ ] Repeat on a patrolling Creature and on a wandering Creature. Confirm each stops for 10 seconds
      and then resumes without a jump or a slide.
- [ ] Place two Creatures, one inside 10 yards of the point and one just beyond it. Confirm only the
      first turns.
- [ ] Pull one Creature into combat first. Confirm it ignores a Distract aimed near it while a calm
      neighbor inside the radius still turns.
- [ ] Cast Distract at a point, wait 5 seconds, then cast at a second point. Confirm the Creature
      turns to the second point and holds for a fresh 10 seconds.
- [ ] Attack a distracted Creature. Confirm it fights at once.

Answer each question in the results. An answer can reverse a decision taken in the implementation.

1. Facing at the end. Does the Creature turn back to its spawn orientation, or stay facing the point?
   The server turns it back, as vmangos does.
2. Visible players. Walk in plain sight, inside aggro range, past a distracted Creature. Does it
   still start its Engagement? The server keeps aggro unchanged.
3. Stealth. Walk stealthed past a distracted Creature at the range it normally detects you. Does it
   notice you? Try the same without Distract for contrast. The server keeps stealth detection
   unchanged.
4. Neutral Creatures. Does a neutral (yellow) Creature inside the radius turn? The server turns only
   hostile Creatures.
5. Immune Creatures. Does a boss, a totem or a sessile Creature turn? The server turns any hostile
   one.
6. Cast visual. Is the destination in the `SMSG_SPELL_GO` target block right, or does the cast need
   another target shape?

Record any missing UI, disconnect, stuck Creature, or Creature that never resumes its patrol. Do not
mark this check complete from headless results alone.
