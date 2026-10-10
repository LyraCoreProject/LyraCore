# Hunter pet live-client check

Run this against a local development realm with an unmodified 1.12.1 build 5875 client. Do not use
a production realm.

1. Log in as a Hunter who knows Tame Beast and has no current pet.
2. Channel Tame Beast on a tameable boar at or below the Hunter's level. Confirm the boar becomes
   the Hunter's pet, the pet bar appears without relogging, and no wild boar respawns immediately.
3. Confirm the pet name, level, XP, happiness and loyalty render without a client error.
4. Set Passive and approach a hostile creature. Confirm the pet does not auto-attack.
5. Exercise Attack, Follow and Stay. Confirm each command changes the pet's behavior.
6. Cast Feed Pet and select valid food from the backpack. Confirm one item is consumed and happiness
   rises. Wrong-diet and too-low-level food must not be consumed.
7. Kill an XP-eligible creature with the pet active. Confirm pet XP rises and any level change
   updates level, health and damage without recreating the pet.
8. Compare white swings while unhappy, content and happy. Confirm the physical damage bands follow
   75%, 100% and 125% of the same base range. Spell damage must not change.
9. Summon an Imp as a Warlock control. Confirm its shared bar, commands, spell casting and physical
   damage are unchanged, and no Hunter care data appears.
10. After the authored boar spawn's normal delay, confirm a wild boar returns independently of the
    tamed pet.

Client-only verdicts are exact bar layout, descriptor rendering, name-query presentation and UI
feedback. Headless tests cover authoritative state, reducer routing, projections and packet bytes;
they do not simulate the build 5875 UI. Record the client build, local realm revision and each step's
result when completing this check.
