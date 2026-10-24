# LyraCore coding standards

These standards govern new and changed code. They do not require cleanup of unrelated legacy code.
Repository configuration and tool output are the source of truth for formatting, lints, and generated
code. `docs/danger-zones.md` remains authoritative for schema changes, publishing, and deployment.

## Order of decisions

Use this order when two good practices compete:

1. Preserve the behavior promised by the Spec.
2. Use the domain language in `CONTEXT.md`.
3. Put authority in the tier described by `docs/architecture.md`.
4. Choose the smallest design that makes the behavior explicit and testable.
5. Verify at the lowest sufficient rung, then run every higher rung the change requires.

## Behavior and scope

- Implement observable behavior, not an imagined future framework. Add a flag, hook, extension point,
  or abstraction only for a current caller or stated requirement.
- Keep one logical change local. Remove a displaced path when the replacement covers it; two ways to
  perform the same operation create drift.
- Prefer a bold simplification when it removes concepts, call paths, or mutable states. Preserve
  complexity only when the domain or protocol requires it.
- Make the failure behavior as deliberate as the success behavior. A gameplay Refusal is an expected
  outcome and stays distinct from a transport or infrastructure failure.

## Domain language and types

- Use `CONTEXT.md` terms in identifiers, tests, comments, logs, docs, commits, and PRs. Update the
  glossary in the same change when a term is introduced or sharpened, and `CORE_TERMS.md` too when
  it lists the term.
- Make illegal states hard to represent. Prefer an enum or a small domain type when it prevents two
  meanings, units, identifiers, or states from being mixed.
- Do not wrap a primitive only to satisfy a style rule. A domain type earns its place by enforcing an
  invariant, clarifying an ambiguous value, or centralizing behavior.
- Prefer exhaustive state transitions over families of booleans and repeated condition chains.
- Give values names that state their domain meaning and units. Avoid generic names such as `data`,
  `manager`, or `helper` when a precise domain term exists.

## Authority and placement

- The native host migration follows the single-Shard boundary in
  `docs/architecture.md#native-host-migration`. New native request handling, AOI, recipient Gates
  and packet construction belong in the Module. The native host handles sockets, cryptographic
  scratch state and the Module's ordered delivery instructions.
- The Module handles request admission, durable state and gameplay rules in one Shard.
- Put code in `lyracore-shared` only when more than one crate genuinely shares the same pure concept or
  calculation. It is not a neutral dumping ground.
- Gameplay partitions use map and Instance. They do not select another Shard.
- Keep the Host delivery types and native reducer arguments in sync with the Module.
  Run the native protocol check after changing this interface.

## Interface depth and seams

- Prefer depth: a small interface should hide substantial behavior, invariants, ordering, and failure
  policy. Callers should express intent, not reproduce the implementation's procedure.
- Apply the deletion test to wrappers and traits. If deleting one only moves its calls unchanged to
  every caller, it is shallow and should normally disappear.
- Introduce a Seam where behavior actually varies. A durable Store plus a Fake is
  a real seam. A trait with one implementation and no concrete substitution is hypothetical.
- Keep internal details behind the interface. Do not expose private operations or state only so a test
  can reach them.
- Pass a dependency through an existing Seam when behavior must vary. Construct ordinary internal
  implementation details where they belong; dependency injection is not a goal by itself.
- When an interface is ambiguous, explore more than one shape before committing. Prefer the shape with
  the most leverage for callers and the best locality for future changes.

## Input, errors, and invariants

- Treat client bytes, operator configuration, and remote state as untrusted input. Return a useful
  error or an explicit fallback instead of panicking at those boundaries.
- In production code, use `unwrap`, `expect`, `panic`, and unreachable-state assertions only for an
  internal invariant that is truly impossible after the boundary has checked its inputs. State the
  invariant when it is not obvious from the types.
- Preserve atomicity around Durable Requests. A Refusal must leave durable state unchanged unless the
  behavior explicitly defines a partial result.
- Log enough stable domain context to act on a failure, without logging secrets or duplicating a full
  state dump.

## Tests

- Test behavior through a caller-facing interface at a named Seam. A test should survive an internal
  refactor that preserves behavior.
- Make the Seam under test explicit before adding a test. If the change introduces or moves a Seam,
  settle that interface before building a large suite around it.
- Develop new behavior in vertical slices where practical: one failing behavior test, the minimum
  implementation that makes it pass, then the next behavior. Review and refactor after the slice is
  green.
- Use a Fake for an owned Store. Substitute a true external system only at its external edge when it
  cannot run locally. Do not fake internal collaborators or assert their call sequence as a substitute
  for an outcome.
- Derive expected values from the Spec, protocol, game data, or a worked example. Do not recompute the
  expected value with the same algorithm as the implementation.
- One test covers one behavior. It may use several assertions when they describe one outcome or prove
  one atomic transition.
- Choose the cheapest test that reaches the real behavior: pure calculation, Store-level behavior,
  codec round-trip, durable integration, Headless Client, or real client. No lower rung proves a
  higher one; use the verification ladder in `docs/architecture.md`.
- Use an Architecture Test for a structural invariant that behavior tests cannot observe reliably.
  Keep its scanner narrow, prove it detects the forbidden mutation, and exclude comments and test-only
  code from lexical checks.
- Add tests for behavior that must remain. Do not preserve a deleted implementation, incidental call
  structure, or every historical bug shape in a permanent regression suite.

## Comments and documentation

- Write code without explanatory comments by default. Let names, types and tests carry ordinary
  mechanics. This applies to source, tests, scripts, configuration and documentation comments such
  as Rust's `///` and `//!`.
- Keep concise interface contracts where names and types leave caller obligations, return values or
  failure behavior unclear. Keep non-obvious local technical facts: opcodes, packet layouts, regexes,
  units, constants, compatibility quirks, and safety, atomicity or ordering constraints.
- Use one short line within the normal line width. Put longer contracts and reasoning in the relevant
  document and leave a short pointer beside the interface. A pointer names the document and when to
  read it. Preserve these pointers when removing nearby explanation; do not flatten an essay onto one
  line or split it into adjacent one-line comments.
- Preserve comments that tools consume, including script metadata, lint directives and editor type
  annotations. Preserve interpreter directives, required attribution and generated-file notices.
  These follow the format their consumer requires, including multiple lines when necessary.
- Remove narration, section dividers, commented-out code, tracker history and descriptions that repeat
  the adjacent code. Record work still needed in the tracker.
- Keep durable design reasoning that needs more than one line in the relevant existing document or
  an ADR. Check it against the code before moving it; deletion is appropriate for stale or redundant
  explanation. Standalone documentation remains part of the project.
- Change generated comments through their generator when we own it. Leave third-party generated
  output alone. Comment-like text in strings and test fixtures is data; preserve its meaning.
- Update or remove a retained comment when behavior changes. Review exceptions for their technical
  content, not merely their length. Apply this policy equally to human and agent contributions.

### Comment review

Before handing off changes or completing a review:

- Inspect the final diff with surrounding code, including new files. Review added and changed comments
  and retained comments whose code changed in source, tests, scripts and configuration.
- Match each reviewed comment to a policy exception and check its claim against the implementation.
  Check that documentation pointers resolve. Remove narration even when it fits on one line.
- Check removed comments for lost contracts, safety constraints, tool annotations and documentation
  pointers. Preserve required meaning in code, a permitted comment or a linked document.
- Repeat the review for subsequent edits. In the handoff, state the scope checked and whether it
  passed, has findings with file locations, or does not apply because no source, tests, scripts or
  configuration changed. Cite retained or relocated contracts when they need reviewer judgment.

Formatting and tests do not establish that a comment is necessary or true; report this review
separately from those checks.

## Completion and review

- Run focused tests while working, type-check regularly, and run the repository's relevant final
  checks before handoff. A focused test passing is not evidence that the suite passes.
- Review the finished diff against both axes: repository standards and the originating Spec. Passing
  one axis does not compensate for failing the other.
- Treat formatting and lint findings as tooling concerns. Human review spends judgment on behavior,
  domain language, authority, depth, seams, safety, and test quality.
- Keep exemptions narrow and explain why the rule does not fit. Delete an exemption when its reason no
  longer exists.
