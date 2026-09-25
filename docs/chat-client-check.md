# Chat client check

Execution status: outstanding. This checklist needs a human with unmodified 1.12.1 build-5875
clients. The Module, Gateway and durable tests prove the chat rules, the packet bytes and the event
rows. They do not prove what the client prints.

Use the local sharded fixture from `./lyracore dev up` after `./lyracore import`: two World Shards
(`lyracore` for Eastern Kingdoms, `lyracore-kalimdor` for Kalimdor), the Instance Pool
(`lyracore-instances`, the Deadmines) and Realm-core (`lyracore-realm`). Never point these steps at
a production realm. Set each client's `realmlist.wtf` to `127.0.0.1` first, because a client may
still point at production.

Prepare three accounts:

- Alice, an Alliance Human in Stormwind (Client A, on `lyracore`).
- Bram, an Alliance Night Elf in Darnassus (Client B, on `lyracore-kalimdor`).
- Grok, a Horde Orc in Orgrimmar (Client C, on `lyracore-kalimdor`). Only the faction steps need
  Client C.

Alice and Bram stand on different World Shards for every step unless a step moves them. Record the
server commit, the client build on the login screen, the character names and their guids. The
quoted texts below are the stock 1.12 FrameXML strings. Guild and officer chat are in
[`guild-client-check.md`](./guild-client-check.md). Raid, raid leader and raid warning chat are in
[`raid-client-check.md`](./raid-client-check.md).

## Party chat across the Shard Boundary

- [ ] Alice invites Bram and Bram accepts. Alice types `/p hello from Stormwind`. Expected: both
      clients print `[Party] [Alice]: hello from Stormwind` once. Alice sees her own line.
- [ ] Bram types `/p hello from Darnassus`. Expected: both clients print it once, with Bram's name.
- [ ] Bram runs `/script SendChatMessage("elune adore", "PARTY", "Darnassian")`. Expected:
      Alice's client shows `[Darnassian]` before the text. Bram's own line shows no tag.
- [ ] Bram leaves the party and types `/p anyone?`. Expected: Bram's client prints
      `You aren't in a party.` Alice gets nothing.
- [ ] Re-form the party. Bram hearths or zones into the Deadmines while Alice sends one `/p` line
      each second. Expected: the lines Alice sends while Bram's loading screen is up never reach
      Bram, and every line after Bram arrives does. Record how many were lost. This gap is known
      and documented in `architecture.md` §5.3.

## Chat Channels

- [ ] Both log in inside their capital. Expected: each client joins `Trade - City` by itself and
      prints `Changed Channel: [2. Trade - City]` or `Joined Channel: [2. Trade - City]`. Record
      the channel number each client shows.
- [ ] Alice types `/2 WTS linen`. Expected: Bram's client prints
      `[2. Trade - City] [Alice]: WTS linen`, although Stormwind and Darnassus are on different
      Shards. Bram answers on `/2` and Alice sees it.
- [ ] Alice types `/chatlist Trade - City`. Expected: the list names both Alice and Bram.
- [ ] Grok joins `Trade - City` in Orgrimmar and types on it. Expected: neither Alice nor Bram sees
      Grok's line, and Grok sees neither of theirs.
- [ ] Alice types `/join lyratest`. Expected: `Joined Channel: [5. lyratest]` or the next free
      number. Bram joins the same name. Expected: Alice's client prints `Bram joined channel.`
- [ ] Alice types `/owner lyratest`. Expected: `[lyratest] Channel owner is Alice.`
- [ ] Bram types `/leave lyratest`. Expected: `Left Channel: [lyratest]` on Bram and
      `Bram left channel.` on Alice.

## Channel moderation

Run these on `lyratest` with Alice as owner and Bram as a member.

- [ ] Alice types `/password lyratest secret`. Expected: `[lyratest] Password changed by Alice.`
      Bram leaves and rejoins with `/join lyratest wrong`. Expected: `Wrong password for lyratest.`
      Bram rejoins with `/join lyratest secret`. Expected: Bram joins.
- [ ] Bram, a plain member, types `/mute lyratest Alice`. Expected: `Not a moderator of lyratest.`
- [ ] Alice types `/mod lyratest Bram`. Expected: both see
      `[lyratest] Moderation privileges given to Bram.` Alice types `/unmod lyratest Bram`.
      Expected: `[lyratest] Moderation privileges removed from Bram.`
- [ ] Alice types `/mute lyratest Bram`, and Bram speaks on the channel. Expected: Bram's client
      prints `[lyratest] You do not have permission to speak.` and Alice sees no line. Alice types
      `/unmute lyratest Bram`, and Bram's next line reaches Alice.
- [ ] Alice types `/ckick lyratest Bram`. Expected: `[lyratest] Player Bram kicked by Alice.`, and
      Bram is off the channel. Bram rejoins.
- [ ] Alice types `/ban lyratest Bram`. Expected: `[lyratest] Player Bram banned by Alice.` Bram
      tries `/join lyratest secret`. Expected: `[lyratest] You are banned from that channel.`
      Alice types `/unban lyratest Bram`. Expected: `[lyratest] Player Bram unbanned by Alice.`
- [ ] With Bram off the channel, Alice types `/cinvite lyratest Bram`. Expected: Bram's client
      prints `Alice has invited you to join the channel 'lyratest'.` and Alice's prints
      `[lyratest] You invited Bram to join the channel`. Alice invites Grok. Expected:
      `Target is in the wrong alliance for lyratest.`
- [ ] Alice types `/ckick lyratest Nobody`. Expected: `[lyratest] Player Nobody is not on channel.`
- [ ] Alice types `/announce lyratest` and `/moderate lyratest`. Expected: the matching
      `Channel announcements disabled by Alice.` and `Channel moderation enabled by Alice.`
      notices, and with moderation on, Bram's line answers `Not a moderator of lyratest.`
- [ ] Alice leaves `lyratest` with Bram on it. Expected: Bram types `/owner lyratest` and sees
      `[lyratest] Channel owner is Bram.` No `Owner changed` line prints, because cmangos announces
      a new owner only when two or more members remain.

## AFK, DND and whisper

- [ ] Alice types `/w Bram hi`. Expected: Bram prints `[Alice] whispers: hi` and Alice prints
      `To [Bram]: hi`.
- [ ] Bram types `/afk Brb`. Expected: Bram's client prints `You are now AFK: Brb`. Alice whispers
      Bram. Expected: Alice prints `To [Bram]: ...` and then `[Bram] is Away From Keyboard: Brb`.
- [ ] Bram types `/p back soon` while AFK. Expected: both clients print
      `[Party] <AFK>[Bram]: back soon`.
- [ ] Bram types `/dnd Busy`. Expected: `You are now DND: Busy.`, and the AFK mark is gone. Alice
      whispers Bram. Expected: `[Bram] does not wish to be disturbed: Busy`.
- [ ] Bram types `/dnd`. Expected: `You are no longer marked DND.`
- [ ] Bram types `/afk`, logs out and logs back in. Expected: Bram comes back without the AFK mark,
      and Alice's next whisper gets no auto-reply.
- [ ] Alice whispers Grok. Expected: `You can only whisper to members of your alliance.`
- [ ] Alice types `/w Nobody hi`. Expected: `No player named 'Nobody' is currently playing.`

## Ignore

- [ ] Bram types `/ignore Alice`. Expected: `Alice is now being ignored.`
- [ ] Alice whispers Bram. Expected: Alice prints `To [Bram]: ...` and then
      `Bram is ignoring you.` Bram prints nothing.
- [ ] Alice speaks on `/2`. Expected: Bram does not see the line.
- [ ] Alice speaks on `/p`. Expected: Bram still sees it. Party chat has no ignore filter in
      vanilla.
- [ ] Bram types `/unignore Alice`. Expected: `Alice is no longer being ignored.`, and whispers
      reach Bram again.

## Friends

- [ ] Alice types `/friend Bram`. Expected: `Bram added to friends.`, and the Friends tab lists
      Bram online with his level, class and `Darnassus`.
- [ ] Bram types `/afk`. Expected: Alice's Friends tab shows Bram as AFK after it refreshes.
- [ ] Bram logs out. Expected: Alice prints `Bram has gone offline.` once. Bram logs back in.
      Expected: Alice prints `[Bram] has come online.` once.
- [ ] Bram travels to the Deadmines. Expected: Alice gets no offline or online notice for the
      Transfer, and the Friends tab shows Bram in the Deadmines' zone.
- [ ] Alice types `/friend Grok`. Expected: `Friends must be part of your alliance.` Alice types
      `/ignore Grok`. Expected: `Grok is now being ignored.`
- [ ] Alice types `/friend Nobody`. Expected: `Player not found.`

## /who

- [ ] Alice types `/who`. Expected: the list shows Bram, although he is on the other Shard, and
      does not show Grok. The chat line reads `[Bram]: Level N Night Elf <class> - Darnassus`.
- [ ] Alice opens the Who frame. Expected: the name, level, class, zone and guild columns render
      correctly for Bram. Record a screenshot.
- [ ] Alice types `/who Bram`, `/who 1-5` and `/who Darnassus`. Expected: each filter keeps or
      drops Bram as its range, name or zone says.

## Say range and the language Gate

- [ ] Alice types `/say hello` in Stormwind. Expected: Bram in Darnassus does not see it.
- [ ] Alice and Bram stand side by side on one Shard. Alice types `/say hello` and `/yell HELLO`.
      Expected: Bram prints `Alice says: hello` and `Alice yells: HELLO`.
- [ ] Alice types `/script SendChatMessage("zug zug", "SAY", "Orcish")`. Expected: nobody hears it.
      The client either refuses the language itself or prints `You don't know that language`.
      Record which.

## Proximity emotes

Put all three characters in Ratchet, on Kalimdor, for these.

- [ ] At about 20 yd, Alice types `/e dances wildly`. Expected: both clients print
      `Alice dances wildly`.
- [ ] At about 20 yd, Alice types `/wave`. Expected: Bram sees the wave line and the animation.
- [ ] At about 30 yd, Alice repeats `/e` and `/wave`. Expected: Bram gets neither.
- [ ] Grok stands 5 yd from Alice. Alice types `/e hops`. Expected: Grok does not see it. Alice
      types `/wave`. Expected: Grok sees the wave.

## Chat Flood Limiter

- [ ] Alice runs `/script for i=1,12 do SendChatMessage("flood "..i, "SAY") end`. Expected: lines
      1 to 11 go out, and the client shows `You must wait 10 Second(s). before speaking again.` for
      line 12.
- [ ] Within the next ten seconds Alice types `/p muted?`, `/2 muted?` and `/wave`. Expected: each
      answers `You must wait N Second(s). before speaking again.` with N counting down, and nobody
      receives anything.
- [ ] Still muted, Alice types `/afk`. Expected: AFK is set, since `/afk` does not count.
- [ ] After ten seconds Alice types `/say back`. Expected: the line goes out.
- [ ] Give Alice a GM level with the Operator's `set_gm_level` on her Home Shard, relog, and run
      the twelve-line script again. Expected: all twelve lines go
      out and no notice appears.

## Addon lines

- [ ] On Bram, run `/script f=CreateFrame("Frame") f:RegisterEvent("CHAT_MSG_ADDON")
      f:SetScript("OnEvent", function() DEFAULT_CHAT_FRAME:AddMessage("["..arg2.."]") end)`.
      On Alice, run `/script SendAddonMessage("LCTEST", " ping ", "PARTY")`. Expected: Bram prints
      `[ ping ]` with both spaces kept.
- [ ] Mute Alice with the flood script, then send the addon message again. Expected: Bram still
      prints `[ ping ]`, since addon lines never count and are never muted.

Record client-side residue a human must look at: the channel number each join printed, the chat
tab and colour each kind lands in, the Who frame columns, and any line that printed twice or not at
all.
