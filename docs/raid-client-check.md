# Raid client check

Execution status: outstanding. This checklist needs a human with unmodified 1.12.1 build-5875
clients. The Module, Gateway and durable tests prove the raid rules, the packet bytes and the event
rows. They do not prove what the client renders.

Use the local sharded fixture from `./lyracore dev up` after `./lyracore import`: two World Shards
(`lyracore` for Eastern Kingdoms, `lyracore-kalimdor` for Kalimdor), the Instance Pool
(`lyracore-instances`, the Deadmines) and Realm-core (`lyracore-realm`). Never point these steps at
a production realm. Set each client's `realmlist.wtf` to `127.0.0.1` first, because a client may
still point at production.

Prepare three accounts with one Alliance character each: Leader (Client A), Member (Client B) and
Far (Client C). Give Leader and Member a level high enough for the Deadmines and set Member's
hearthstone home to Stormwind. Keep ten playerbots or extra clients ready to fill a Raid of 12.
Record the server commit, the client build on the login screen, the character names and the guids.

## Convert to a Raid

- [ ] Leader invites Member, Member accepts. Leader right-clicks its portrait and picks Convert to
      Raid. Expected: both clients show the raid tab with both characters in Group 1, and Leader gets
      no error text.
- [ ] Leader tries Convert to Raid again. Expected: nothing changes and no error text appears.
- [ ] Leader invites the playerbots until the Raid holds 12. Expected: the first ten members sit in
      Groups 1 and 2, the next ones in Group 3, and every client's raid tab lists the same roster.

## Subgroups

- [ ] Leader drags Member from Group 1 to Group 4 in the raid tab. Expected: every client shows
      Member in Group 4, and Group 1 has one member less.
- [ ] Leader fills Group 2 to 5 members, then drags a sixth member onto it. Expected: the move is
      refused and the roster does not change.
- [ ] Leader drags Member onto a member of Group 2. Expected: the two members trade Groups on every
      client in one step.
- [ ] Member, as a plain member, tries the same drag. Expected: nothing changes.

## Leadership and Assistants

- [ ] Leader promotes Member to Assistant. Expected: every raid tab shows Member's Assistant mark.
- [ ] Member, as an Assistant, invites a character outside the Raid, who accepts. Expected: the
      character joins the first Group with room.
- [ ] Member kicks that character. Expected: it leaves the Raid on every client.
- [ ] Member tries to kick Leader. Expected: nothing changes.
- [ ] Member invites a character, then Leader demotes Member before the invite is accepted. The
      character accepts. Expected: the character joins the Raid, as in cmangos.
- [ ] Leader promotes Member again. Member invites a character, then leaves the Raid. The character
      accepts. Expected: the character joins the Raid, and no Party with Member forms.
- [ ] Away from the Raid, one character forms a Party with a second, invites a third, and the
      Party disbands before the third accepts. The third accepts. Expected: the character does not
      join, no group forms, and no message appears.
- [ ] An ungrouped character invites another, then accepts an invite into the Raid before its own
      invite is answered. The other character accepts. Expected: the character does not join, no
      group forms, and no message appears.
- [ ] Leader invites an ungrouped character who has an unanswered invite out to someone else.
      Expected: Leader sees "is already in a group" and no invite reaches the character.
- [ ] Member rejoins. Leader passes the lead to Member. Expected: every client prints "Member is
      now the group leader" before the raid tab moves the leader crown.
- [ ] Member passes the lead back to Leader. Leader passes the lead to a member who is offline.
      Expected: the lead does not move.
- [ ] With Member as the first Assistant, Leader leaves the Raid. Expected: every client names
      Member as the new leader. Leader rejoins through Member's invite.

## Ready Check

- [ ] Leader starts a Ready Check. Expected: every member, Leader included, sees the Ready Check
      dialog.
- [ ] Member answers Ready and a playerbot answers Not Ready or times out. Expected: Leader sees each
      answer, and the other members see only their own dialog close.
- [ ] A plain member starts a Ready Check. Expected: nothing happens on any client.
- [ ] Promote Member to Assistant. Member starts a Ready Check. Expected: every member sees the
      dialog.

## Target Icons

- [ ] Leader marks a creature with Skull. Expected: every client shows Skull over that creature.
- [ ] Leader marks the same creature with Cross. Expected: Skull clears and Cross appears, on every
      client.
- [ ] A plain member tries to mark. Expected: nothing changes. Member, as an Assistant, marks a
      second creature with Moon. Expected: every client shows Moon.
- [ ] With three icons set, Client B logs out and back in. Expected: after the raid tab loads,
      Client B shows every held icon, and no icon that was never set.
- [ ] Disband to a Party of Leader and Member, form it again and set two icons. Leader changes the
      loot method. Expected: both clients keep both icons after the new group list.
- [ ] In that Party, Leader passes the lead to Member. Expected: both clients keep both icons.
- [ ] In that Party, Member logs out and back in. Expected: Member's client shows both icons again.
- [ ] In that Party, Member crosses from Eastern Kingdoms to Kalimdor by boat or portal, which
      changes World Shard. Expected: after the loading screen, Member's client shows both icons
      again.

## Minimap ping, `/roll` and the cooldown

- [ ] In the Raid, Leader clicks the minimap. Expected: every other member sees the ping at the same
      spot. Leader's own client shows its normal local ping.
- [ ] Ungrouped, a character pings. Expected: no other client sees it.
- [ ] In the Raid, Leader types `/roll`. Expected: every member, Leader included, sees "Leader rolls
      N (1-100)". A character outside the Raid standing next to Leader sees nothing.
- [ ] Ungrouped, a character types `/roll 5 10`. Expected: only that character sees the result, in
      the range 5 to 10.
- [ ] Leader runs a macro with `/roll` on two lines. Expected: every member sees exactly one roll
      result. A third `/roll` one second later shows normally.
- [ ] Leader clicks the minimap twice in under a second. Expected: the other members see one ping.
- [ ] Leader runs `/script DoReadyCheck() DoReadyCheck()`. Expected: members see one dialog.
- [ ] Leader marks three creatures in under a second. Expected: all three icons appear, because
      Target Icons have no cooldown.

## Chat

- [ ] Member types `/ra hello`. Expected: every member sees it in the raid color.
- [ ] Leader runs `/script SendChatMessage("hello", "RAID_LEADER")`. Expected: every member sees it
      as raid leader chat. A plain member runs the same. Expected: nothing is sent.
- [ ] Leader types `/rw pull`. Expected: every member sees the center-screen raid warning. An
      Assistant's `/rw` works. A plain member's `/rw` is not sent.
- [ ] Put Member in Group 4 and Leader in Group 1. Member types `/p hi`. Expected: only Group 4
      sees it. Leader does not.
- [ ] In a Party of Leader and Member, `/ra hello`. Expected: nothing is sent.
- [ ] On Client B, run
      `/script local f=CreateFrame("Frame") f:RegisterEvent("CHAT_MSG_ADDON") f:SetScript("OnEvent", function() DEFAULT_CHAT_FRAME:AddMessage(arg1.." "..arg2.." "..arg3.." "..arg4) end)`.
      On Client A, run `/script SendAddonMessage("LCTEST", "raid-ping", "RAID")`. Expected: Client
      B prints `LCTEST raid-ping RAID Leader` once, and nothing shows in the chat window as a
      normal line.

## Quest credit and loot in a Raid of 12

- [ ] With the Raid at 12 members, every member in range of one creature, and every member on an
      ordinary quest that needs that creature: the Raid kills it. Expected: no member gets quest
      credit, as in vanilla. Nobody sees the objective count go up.
- [ ] Leave the Raid, form a Party of Leader and Member, and kill the same creature. Expected: both
      get credit.
- [ ] In the Raid of 12, kill a creature that drops loot, with the loot method on Group Loot.
      Expected: a member in Group 3 can open the corpse, and the loot roll window reaches every
      member in range.
- [ ] A member on the ordinary quest opens the corpse of a quest-item creature in the Raid.
      Expected: the quest item does not show in the loot window.

## Member stats across shards

- [ ] Keep Leader in Eastern Kingdoms and move Far to Kalimdor. Expected: the raid tab shows Far as
      online, with its zone.
- [ ] Far takes damage and spends mana. Expected: within about 5 s, Leader's raid frame for Far
      shows the new health and mana.
- [ ] Far gains a buff and a debuff, and a Hunter member summons and dismisses a pet. Expected:
      Leader's frames show the buff, the debuff and the pet frame, then the pet frame clears.
- [ ] Member walks out of Leader's view on the same World Shard. Expected: Leader's party or raid
      frame keeps updating Member's health, and Member's position shows on the map.
- [ ] Far logs out. Expected: Leader's frame for Far goes to offline once.
- [ ] Split the Raid: Member enters the Deadmines, Leader stays in Eastern Kingdoms and Far in
      Kalimdor. Expected: every client lists the same roster, stats frames update, and `/ra`, a
      minimap ping and `/roll` reach all three.

## Instance Removal countdown

Use a Party of three (Leader, Member and one more) inside the Deadmines. The Raid cannot enter
because the dungeon entry cap stays at 5.

- [ ] Inside the Deadmines, Member leaves the Party. Expected: Member's client shows the 60 s
      removal countdown. After 60 s, Member is at its hearthstone home in Stormwind, in the open
      world.
- [ ] Leader kicks Member inside the Deadmines. Expected: the same countdown and removal.
- [ ] Member leaves, then accepts a new invite to the same Party inside 60 s. Expected: the
      countdown popup hides and Member stays in the Deadmines.
- [ ] In a Party of two inside the Deadmines, Member leaves, which disbands the Party. Expected:
      both clients show the countdown. Leader forms a new Party with Member inside the dungeon
      before 60 s. Expected: both countdowns hide and both stay.
- [ ] Member leaves inside the Deadmines, then walks out through the portal before 60 s. Expected:
      after the loading screen the countdown popup is gone, and Member is not teleported later.
- [ ] In each hide above, watch the chat window and the error text area. Expected: the hide, sent
      with error code 1 and timer 0, shows no "raid group required" text or any other stray line.
- [ ] Inside the Deadmines, Leader converts the Party of three to a Raid and promotes the third
      member to Assistant. Expected: no countdown appears.
- [ ] Leader moves a member to another Subgroup. Expected: no countdown appears.
- [ ] Member leaves the Raid, and the Assistant's invite takes it back inside 60 s. Expected: the
      countdown starts, then hides, and Member stays.

Record for each step the result, the time, and any visible failure, error text or disconnect.
