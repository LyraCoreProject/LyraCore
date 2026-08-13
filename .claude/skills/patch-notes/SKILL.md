---
name: patch-notes
description: Draft a community-Discord patch-notes post from merged pull requests.
disable-model-invocation: true
---

# Patch notes

Turn merged pull requests into a post for the community Discord.

The reader plays on a LyraCore realm or runs one. They never read the code. Every note states the
**effect** — what is different the next time they log in — and the effect is all it states.

## Steps

1. **Fix the range.** Use the range the user names: a date, a PR number, "since the last post".
   Given no range, take pull requests merged in the last 7 days and say so in the reply.

2. **List the merged pull requests in the range.**
   `gh pr list --state merged --search "merged:>=YYYY-MM-DD" --limit 100 --json number,title,body,mergedAt`

3. **Name each pull request's effect** in one sentence its reader would recognise. Read the body
   first. When the body describes only mechanism, read the commits before you decide —
   `gh pr view <N> --json commits` and `git log --format=%B <sha>`. A title alone is a guess, and a
   guessed effect is the failure this step exists to catch.

4. **Sort every pull request into one pile:**
   - `player` — changes what a character can do, see, or hold in the game.
   - `operator` — changes how a realm is built, published, configured, or upgraded.
   - `dropped` — no difference either reader can observe: refactors, tests, contributor docs,
     regenerated bindings, work that only completes an earlier note.

   The three piles together hold every pull request from step 2, each exactly once. Count them
   against step 2's list before you write.

5. **Write the post** to the Format and Voice below.

6. **Reply with the post, then the dropped list** — PR number and title, one line each — so the
   user can pull anything back in.

## Format

Order, top to bottom:

- A title line: `LyraCore — <month and year, or the build name the user gives>`.
- Two sentences at most, naming the headline change. Nothing else in the opener.
- One `##` section per part of the game the notes touch, named as the game names it: `Mail`,
  `Bank`, `Movement`. A part earns a section at two notes or more; single notes collect under
  `General`.
- `## Fixes` — repairs to behaviour that already shipped.
- `## For server operators` — last, and only when the operator pile is non-empty. Upgrade steps
  and anything that changes on publish go here.

Each note is one `-` bullet of one sentence. Sections run 3 to 8 bullets; a longer section is two
parts of the game wearing one name, so split it.

Discord renders `##` and `-` and caps a message at 2000 characters. Over that, split at a section
boundary and label the pieces `(1/2)`, `(2/2)`.

## Voice

- Present tense, active, one idea per bullet: "Mail now carries items and copper."
- State the new behaviour. Reach for the old one only where the reader built a habit on it.
- Use the words the game uses: mailbox, bank, bag, copper, innkeeper, flight master.
- Keep every number exact — slot counts, costs, timers, ranges.
- Keep tracker numbers, file paths, crate names, and function names out of the post. The dropped
  list is where numbers belong.

Some words in the repo have a plain equivalent; the rest have none, and a note that needs one is a
note about something the reader cannot see:

| in the repo | in the post |
| --- | --- |
| reducer, module, SpacetimeDB | the server |
| shard, world database | the realm |
| gateway | the connection to the realm |
| escrow, fence, commit | the outcome only: "your copper is never lost part-way through a trade" |
| binding, codec, opcode, SMSG/CMSG | drop the note |

A worked pair — the pull request said:

> `CMSG_SEND_MAIL` has always carried `cash_on_delivery_amount` and `game_mail` has always had a
> `cod` column; nothing ever set it.

The note says:

> - You can now put a price on a mailed item, and you are paid when the buyer takes it.
