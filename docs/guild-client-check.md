# Guild client check

Execution status: outstanding. This checklist needs a human with two unmodified 1.12.1 build-5875
clients. The automated suite covers the Module rules, the Gateway replies and the wire bytes. It
cannot see what the client draws. Do not mark a step done from headless results alone.

Date run: ______ Server commit: ______ Client build shown at login: ______

## Fixture

- A local stack from `./lyracore dev up`, in the default four-database topology so Realm-core is its
  own database (`lyracore-realm`). Never use `--single` for this check, and never point it at a
  production database.
- Before either client starts, set its `realmlist.wtf` to `set realmlist 127.0.0.1`. The local
  clients point at the production realm by default.
- Imported world data, so the Deadmines instance, a Tabard Designer and a Petitioner that sells
  Guild Charters (item 5863) exist.
- Three Alliance Characters on three Accounts:
  - GM, with a GM level above 0.
  - Alpha, level 20 or more, in the open world in Stormwind, next to a Tabard Designer, with at
    least 11 gold.
  - Beta, level 20 or more, inside the Deadmines, so it lives on the Instance Pool.
- For step 6, nine more Alliance Characters on nine more Accounts, or the maintainers' Headless
  Client to drive them.

For each step, write what the client showed on the "Observed" line.

## Procedure

1. Founding
   - [ ] As GM, say `.guild create Alpha "Boundary Test"`.
     Expected: no system line for GM. Alpha's name plate shows `<Boundary Test>` and its guild
     window shows Alpha as Guild Master.
     Observed:

2. Invite across the Shard Boundary
   - [ ] As Alpha, `/ginvite Beta`. Expected: Beta, inside the Deadmines, gets the invite popup
     naming Alpha and Boundary Test.
     Observed:
   - [ ] As Beta, accept. Expected: both clients show "Beta has joined the guild". Beta's name plate
     shows `<Boundary Test>`. Both guild windows list both members, online, with the right level
     and zone: Stormwind for Alpha, The Deadmines for Beta.
     Observed:
   - [ ] As Alpha, `/who Beta`. Expected: the Who list shows Beta with `<Boundary Test>`.
     Observed:
   - [ ] As a third Alliance Character with a pending guild invite, right-click and sign any open
     Charter. Expected: "<own name> has already been invited to a guild". Offering a Charter to that
     Character answers the same line with its name.
     Observed:

3. Guild chat, MOTD and ranks
   - [ ] `/g` from Alpha, then `/g` from Beta. Expected: each line reaches the other client in the
     guild chat colour.
     Observed:
   - [ ] As Alpha, `/gmotd Across the boundary`. Expected: both clients print the new message of the
     day.
     Observed:
   - [ ] As Alpha, `/gpromote Beta`, then `/gdemote Beta`. Expected: both clients print the promotion
     and the demotion with the rank names, and Beta's rank in the roster follows.
     Observed:
   - [ ] As Alpha, `/gleader Beta`. Expected: both clients print that Beta is the new Guild Master.
     Alpha shows as Officer. Pass it back with `/gleader Alpha` from Beta.
     Observed:
   - [ ] As Beta, try to join the GuildRecruitment channel in a city. Expected: nothing happens and
     Beta is not listed in the channel. A Character outside every guild joins it normally.
     Observed:

4. Transfer
   - [ ] Walk Beta out of the Deadmines to the open world, so it crosses to a World Shard. Watch
     Alpha's guild window during the loading screen. Expected: Beta stays listed online the whole
     time. After arrival Beta's name plate still shows `<Boundary Test>`, and the roster shows the
     new zone.
     Observed:

5. Tabard emblem
   - [ ] As Alpha at the Tabard Designer, design an emblem and save it. Expected: the window closes,
     10 gold leaves Alpha's purse, and Alpha's tabard shows the new design.
     Observed:
   - [ ] On Beta, with a guild tabard equipped. Record what the client does with TABARD_CHANGED
     followed by the guild query response: does Beta's tabard, and Alpha's tabard as Beta sees it,
     re-render without a relog? mangos never sends TABARD_CHANGED, so this behaviour is new.
     Observed:
   - [ ] As Beta (not the Guild Leader), try to save an emblem. Expected: the "only the guild master"
     error, and no copper leaves Beta's purse.
     Observed:

6. Charter path
   - [ ] As a Character outside every guild, buy a Guild Charter named "Charter Check" from the
     Petitioner. Expected: 10 silver leaves the purse and the Charter appears in the bags.
     Observed:
   - [ ] Offer it to the nine signers and have each sign. Expected: each signature shows in the
     owner's Charter window, and a second Character on the same Account is told it already signed.
     Observed:
   - [ ] Turn the Charter in at the Petitioner. Expected: "Charter Check" is founded, the Charter
     leaves the bags, and each signer gets the founder message and shows `<Charter Check>`.
     Observed:

7. Deletion
   - [ ] Add a third member, Gamma, to Boundary Test and log it out. At character select on Gamma's
     Account, delete Gamma. Expected: the deletion succeeds. Within a few seconds Alpha's and Beta's
     clients print "Gamma has left the guild" and the roster no longer lists Gamma.
     Observed:
   - [ ] At character select on Alpha's Account, try to delete Alpha while it leads Boundary Test.
     Expected: "Character deletion failed", and Alpha is still listed on the character screen and in
     the guild.
     Observed:
   - [ ] Deletion the Gateway did not see. Make Delta the Guild Leader of a second guild with one
     other member, Echo, and log Delta out. Stop the Gateway. On the local stack only, delete Delta
     on its World Shard with
     `spacetime call -s local <shard> delete_character <account_id> '{"guid":<guid>,"ownership":null}'`.
     Start the Gateway and log Echo in. Expected: Echo's guild window shows Echo as Guild Master and
     no Delta. Log Echo out and delete it the same way. Expected: the guild is gone, so its name is
     free: as GM, `.guild create GM "<its name>"` succeeds.
     Observed:

Record any missing UI, wrong string, duplicate message, stale name plate, disconnect or client
error. Flag anything that needs a second look as outstanding here rather than claiming it.
