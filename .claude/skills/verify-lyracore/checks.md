# Real-client checks

Each section lists only what a person must see on an unmodified 1.12.1 build 5875 client. The tests
and `native-host/check.py` already cover Module rules, packet bytes and durable rows, so these checks
cover what the client draws and prints.

## Standard setup

- Use a local Realm from `dev up` with content imported (`./lyracore import`), never a deployed one.
- Point each client's `realmlist.wtf` at `127.0.0.1` ([quickstart](../../../docs/quickstart.md#5-point-your-client-at-it)).
  Create the Accounts with `./lyracore account create`. Grant GM only where a section says so.
- `call` below is `spacetime --config-path .lyracore/native-cli.toml call --server
  http://127.0.0.1:3000 lyracore`, followed by a debug reducer and its arguments.
- Record the Core commit, the client build on the login screen, and for each step what the client
  showed. A missing control, wrong string, duplicate line, stale name plate or disconnect is a finding.
  Headless output never completes a step.

## Auction house

Tests: `auction_buyout.rs`, `auction_cancel.rs`, `auction_expiry.rs`. Scenario: `check_auctions`.

Setup. Seller and Bidder with recorded copper and two distinct mailable stacks, one auctioneer
within 10 yards. Record the auctioneer and the house rates.

- List both stacks for 8 hours (the client offers 2, 8 and 24), one with a buyout. The owner tab
  shows both and Seller lost only the deposits.
- Bidder's browse and name search show the rows, stack details, prices, owner and time left. A bid
  shows on the bidder tab with the exact copper debit. An offer above the buyout charges the buyout.
- At the mailbox, each outcome mails the subject "Outbid on `<item>`", "Auction won: `<item>`",
  "Auction successful: `<item>`", "Auction expired: `<item>`" or "Auction cancelled: `<item>`",
  sent by the house, with no Return button. Won and successful invoices name the price.
- Cancelling an unbid listing returns the item by mail and leaves copper unchanged. Cancelling a
  bid listing costs Seller the Auction Cut and mails Bidder the full bid. With copper below the cut,
  the client shows no error and the listing stays.
- A second online client's chat prints `ERR_AUCTION_OUTBID_S`, `ERR_AUCTION_WON_S`,
  `ERR_AUCTION_SOLD_S`, `ERR_AUCTION_EXPIRED_S` and `ERR_AUCTION_REMOVED_S` in that order.
- After a relog, no item, proceeds, refund or listing appears twice. A Stormwind listing shows from
  Ironforge and Darnassus. A Booty Bay listing shows at none of the three.

## Mail

Tests: `mail_delivery.rs`, `mail_expiry.rs`, `mail_letter_copy.rs`, `reward_letter.rs`. Scenario:
`check_mail`.

Setup. A Character on Account A, one on Account B and an alt on Account A, each with copper and a
mailable stack.

- Mail with an item between the Account A Characters, and money-only mail to Account B, arrive at
  once with the new-mail envelope and a toast. Item mail to Account B arrives about an hour later,
  also with envelope and toast.
- A read mail still shows read after you close the window, walk away and reopen. Three mails list
  newest first.
- A cash on delivery payment reaches the sender as "COD Payment: `<subject>`" with the prefix once.
  A returned mail shows as returned and offers Delete, not Return.
- Run `call debug_stage_mail_expiry_fixture`, then give its "timer: returns" letter to your
  Character with `UPDATE game_mail SET recipient_guid = <guid> WHERE subject = 'timer: returns'`.
  The mail shows as returned, with its item, and offers Delete.
- A Letter Copy ("Make Permanent Copy") still reads correctly after you delete the source mail,
  mail it, trade it and sell it at the auction house.
- Turn in a quest that sends a Reward Letter, for example 3644 to 3647, and wait out the delay. The
  inbox shows the quest giver as sender, the template text once with `$B` and `$n` filled in, the
  attached item and no Return button. `call debug_stage_reward_letter_fixture` stages the copper
  variant, which shows no toast.

## Meeting stone

Tests: `meeting_stone_queue.rs`, `meeting_stone_matching.rs`. Scenario: `check_meeting_stones`.

Setup. Import the `gameobjects` family, or `game_meeting_stone` is empty and every stone is silent.
Leader and Seeker are Alliance Characters inside the Deadmines stone's level range, Low is below
it, and Leader can invite three playerbots. Use the Deadmines stone in Westfall.

- The tooltip shows the level range. Low's right-click gets the client's own level message.
- Seeker's right-click shows the minimap button, a tooltip naming The Deadmines and the joined
  line. The dialog on the button leaves the queue with the left-queue line. The button survives the
  Deadmines portal and is gone after a relog.
- Leader as a non-leader, as a Raid leader and with a full Party each get their failure line.
- Queue a Party, then change it. A member who leaves loses the button, and Leader sees the
  member-left line and keeps its button. A kicked member sees the looking-for-a-new-party line and
  keeps a button, and Leader sees the member-removed line and loses its button. Passing the lead
  prints nothing and both keep the button. Converting to a Raid drops everyone's button with the
  left-queue line. After 5 minutes every member sees the in-progress line.
- A stone-formed Party shows the member-added line, then the complete line with the button gone.
  Seeker's party frame lists five members, `/p` reaches both clients and a kill splits XP.

## Self-resurrection

Tests: `self_resurrection.rs`. Scenario: `check_death`.

Setup. A Warlock of level 18 or higher and a Priest who knows Resurrection. Import spells and items
so the Soulstone ranks exist. The field index and the 30 minute duration come from emulator source
and the ClassicDB dump, not a client, so these steps are their only client proof.

- `call debug_grant_item <guid> 5232 1` and use the Minor Soulstone on the Warlock. Record the
  cast bar and whether the buff shows 30 minutes.
- `call debug_set_health <guid> 0`. The death dialog shows a second button beside Release Spirit.
  Record its text. Clicking it closes the death screen with 400 health, 700 mana (or the maximum if
  lower), no Resurrection Sickness and no corpse. Record any cast animation or sound.
- Release Spirit first, and record whether the ghost gets any prompt. After a Priest resurrects the
  Warlock, a death without a Soulstone shows only Release Spirit. Log out on the death screen and
  back in, and record the state. Moving during the 3 second cast still consumes the item.

## Distract

Tests: `distract.rs`. No `check.py` scenario.

Setup. A Rogue of level 22 or higher with 30 energy who knows Distract. Spell 1725 must exist in
the imported spells. Stand near idle, patrolling and wandering Creatures.

- The cursor becomes a ground reticle. A point 30 yards away is accepted and a farther one gives
  the out-of-range message.
- A cast 5 yards from an idle Creature costs 30 energy, shows a 30 second cooldown and keeps the
  Rogue stealthed. The Creature turns toward the point, holds for 10 seconds by stopwatch, then
  turns back. Record whether it snaps or turns smoothly.
- A patrolling or wandering Creature stops for 10 seconds and resumes without a jump or slide.
- Only Creatures within 10 yards turn. A Creature in combat ignores it. A second cast restarts the
  10 seconds. Attacking a distracted Creature starts the fight at once.
- Record the cast visual (destination or caster, no impact flash on the Rogue) and what a visible,
  a stealthed, a neutral, a boss and a totem Creature do. The server turns only hostile Creatures
  and leaves aggro and stealth detection unchanged.

## Duel

Tests: `duel_killing_blow.rs`. Scenario: `check_duels`.

Setup. Import spell 7266 and its duel-flag gameobject. Two living same-faction Characters on one map.

- The Duel action puts one flag at the midpoint and shows the request UI. Both clients show the
  3 second countdown, and attacks stay blocked until it ends.
- Moving beyond 50 yards from the flag shows the out-of-bounds warning once. Returning within 10
  seconds clears it once. Staying out 10 seconds announces the other side as winner, removes the
  flag and ends combat.
- `/forfeit` after the countdown prints the fled winner text and clears both clients' Duel state.
- A would-be lethal hit leaves the loser at exactly 1 health with no Release Spirit and no corpse.
- Logout during a Duel gives the other client one interruption message and no stale flag.

## Guild

Tests: `guild_core.rs`, `guild_membership.rs`, `guild_chat.rs`, `guild_settings.rs`,
`guild_petition.rs`, `guild_cleanup.rs`. Scenarios: `check_guild`, `check_petitions`.

Setup. GM (GM level above 0), Alpha (level 20 or higher, in Stormwind at a Tabard Designer, 11
gold or more) and Beta (level 20 or higher, inside the Deadmines), each on its own Account. A
Petitioner that sells Guild Charters (item 5863). The Charter step needs nine more Characters or
Headless Clients.

- GM says `.guild create Alpha "Check Guild"`. Alpha's name plate shows `<Check Guild>` and the guild
  window shows Alpha as Guild Master.
- `/ginvite Beta` shows the popup naming Alpha and the guild. After acceptance both print "Beta has
  joined the guild", the roster shows Stormwind and The Deadmines, and `/who Beta` shows the guild.
- `/g`, `/gmotd`, `/gpromote`, `/gdemote` and `/gleader` reach both clients with the rank names.
- Beta leaving the Deadmines stays listed online through the loading screen and keeps its name
  plate. A tabard emblem costs 10 gold and updates the tabard. Record whether the other client
  redraws tabards without a relog. A non-leader gets the "only the guild master" error.
- A Charter costs 10 silver. Each signature shows in the owner's window, and a second Character on a
  signer's Account is told it already signed. Turning it in founds the guild for every signer.
- Deleting a member at character select makes the others print "Gamma has left the guild". Deleting
  the leader fails with "Character deletion failed".

## Chat

Tests: `realm_chat.rs`, `chat_channels.rs`, `chat_channel_moderation.rs`, `away_and_whisper.rs`,
`friends.rs`, `proximity_emotes.rs`. Scenarios: `check_chat`, `check_channels`, `check_social`,
`check_addon`.

Setup. Alice (Alliance Human, Stormwind), Bram (Alliance Night Elf, Darnassus) and Grok (Horde
Orc, Orgrimmar) on three clients. Guild and officer chat are under Guild, raid chat under Raid.

- Party chat prints `[Party] [Alice]: hello` once on both clients. A Darnassian line, sent with
  `/script SendChatMessage("elune adore", "PARTY", "Darnassian")`, shows `[Darnassian]` on the
  listener only.
- Each client joins `Trade - City` by itself and prints the channel number. A line on `/2`
  reaches the other city but not Horde. `/join`, `/owner`, `/leave`, `/password`, `/mod`, `/mute`,
  `/ckick`, `/ban`, `/cinvite` and `/announce` print the stock notices, and a refusal prints its
  own line, such as "Not a moderator" or "You are banned from that channel."
- `/afk`, `/dnd` and whisper print the auto-reply lines and the `<AFK>` mark. A cross-faction
  whisper and an unknown name print their error lines. `/ignore` hides channel lines but not party.
- The Friends tab shows level, class and zone. Online and offline notices print once.
- `/who` lists Characters of both cities, not the other faction, and the Who frame renders every
  column. `/say` stays local. Proximity emotes reach about 20 yards, not 30.
- Twelve rapid `/say` lines print "You must wait N Second(s). before speaking again." and mute `/p`, `/2`
  and `/wave`, but not `/afk` or addon lines. A GM level removes the limit.

## Raid

Tests: `raid_convert.rs`, `raid_subgroups.rs`, `raid_leadership.rs`, `raid_broadcasts.rs`,
`raid_chat.rs`, `raid_quest_credit.rs`, `instance_removal.rs`. No `check.py` scenario.

Setup. Leader, Member and Far on three clients, with ten playerbots or extra clients to fill 12
slots. Set Member's hearthstone to Stormwind and give Leader and Member a Deadmines level.

- Convert to Raid shows the raid tab with Groups 1 to 3 filled by count and the same roster on
  every client. Dragging between Subgroups, a swap and a full Group each match the rules.
- Assistant marks, the leader crown, "is now the group leader" and Ready Check dialogs render on
  every client. Target Icons show on every client and return after a relog.
- A minimap ping, `/roll`, `/ra`, `/rw` (center-screen warning) and raid leader chat reach every
  member once. A Subgroup member's `/p` reaches only its Group.
- In a Raid of 12 a shared kill gives no quest credit, and the quest item stays out of the loot
  window. Group Loot rolls reach every member in range.
- Raid frames show health, mana, buffs, pet and offline state for a member in another zone, within
  about 5 seconds.
- Leaving a Party inside the Deadmines shows the 60 second Instance Removal countdown, then the
  hearthstone teleport. Rejoining hides the popup with no stray error text.

## Hunter pet

Tests: `hunter_pet.rs`. Scenario: `check_pets`.

Setup. A Hunter who knows Tame Beast and has no pet, next to a tameable boar at or below its level.

- Channel Tame Beast. The boar becomes the pet, the pet bar appears without a relog and no wild
  boar respawns at once. Name, level, XP, happiness and loyalty render without a client error.
- Passive stops auto-attack. Attack, Follow and Stay each change behavior.
- Feed Pet consumes one valid food and raises happiness. Wrong-diet and too-low-level food stay.
- Pet XP rises on a kill and a level-up updates level, health and damage without recreating the pet.
- White swing damage follows 75%, 100% and 125% of one base range while unhappy, content and happy.
  A Warlock Imp keeps its shared bar and shows no Hunter care data. The wild boar returns after the
  spawn delay.

## Mount

Tests: `mount.rs`. Scenario: the mount block in `check_world_objects`.

Setup. The reins are item 5090054 (spell 50310, +60% speed, riding skill 75 required). Run
`call debug_grant_item <guid> 5090054 1`. For the trainer, run `call debug_seed_scenario_fixtures`,
then `call debug_spawn_at_feet <guid> 51007 3.0` and keep 100 copper. Otherwise skip it with
`call debug_learn_riding <guid> 75`.

- Without riding skill the reins give a training refusal and change nothing. Buying the Apprentice
  rank shows Riding 75/75.
- Using the reins renders the horse, shows the buff and is visibly faster (11.2 yards per second,
  not 7.0). Right-clicking the buff off reverts the model and the speed in one frame.
- Melee, spell, periodic, fall and environmental damage leave the mount up. An auto-attack or an
  active cast dismounts. `call debug_cast_at <other> 50311 <rider>` dismounts and slows the rider.
- The error frame shows "you cannot mount while dead", "while in combat", "indoors" and "in deep
  water". The indoor refusal and the doorway dismount need a vmap generation verified after the
  indoor table existed, so run `call verify_vmap_generation <id>` again on an older one.
- A relog returns the Character mounted at mounted speed. An observer sees the same model and
  speed with no relog. A taxi flight refuses while mounted, and landing leaves no mount state.

## Taxi

Tests: `typed_taxi_contracts.rs`. Scenario: the flight master block in `main()` of `check.py`.

Setup. A Character at a flight master, with the destination node discovered.

- The taxi map opens, the flight follows a multi-point path with the flight animation, and landing
  clears the mount at the destination.
- Fly the route once per faction. The mount on screen matches `mount_display_horde` or
  `mount_display_alliance` in that route's `game_taxi_node` row. Record both node ids.
- The session stays stable through takeoff, flight and landing.
