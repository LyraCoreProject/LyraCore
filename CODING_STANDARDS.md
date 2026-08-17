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
  glossary in the same change when a term is introduced or sharpened.
- Make illegal states hard to represent. Prefer an enum or a small domain type when it prevents two
  meanings, units, identifiers, or states from being mixed.
- Do not wrap a primitive only to satisfy a style rule. A domain type earns its place by enforcing an
  invariant, clarifying an ambiguous value, or centralizing behavior.
- Prefer exhaustive state transitions over families of booleans and repeated condition chains.
- Give values names that state their domain meaning and units. Avoid generic names such as `data`,
  `manager`, or `helper` when a precise domain term exists.

## Authority and placement

- Follow `docs/architecture.md` for tier ownership. The Module owns durable state and gameplay rules.
  The Gateway owns protocol handling, routing, relays, and only the realm-wide Gates a single shard
  cannot answer.
- Gateway handlers map client intent to Durable Reads or Durable Requests, then map durable outcomes
  to client messages. Those operations cross a Store seam; gameplay rules do not grow inside packet
  handlers.
- Put code in `lyracore-shared` only when more than one crate genuinely shares the same pure concept or
  calculation. It is not a neutral dumping ground.
- The Module stays shard-agnostic. Code that needs a shard name or topology decision belongs in the
  Gateway unless `docs/architecture.md` explicitly identifies an exception.
- Treat generated bindings as generated code. Follow `docs/danger-zones.md` for regeneration and its
  documented exceptions.

## Interface depth and seams

- Prefer depth: a small interface should hide substantial behavior, invariants, ordering, and failure
  policy. Callers should express intent, not reproduce the implementation's procedure.
- Apply the deletion test to wrappers and traits. If deleting one only moves its calls unchanged to
  every caller, it is shallow and should normally disappear.
- Introduce a Seam where behavior actually varies. In Gateway code, a production Store plus a Fake is
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

- Comments explain why a constraint exists, what an interface promises, or which non-obvious protocol
  fact controls the code. Let names and types explain ordinary mechanics.
- Keep interface comments concise but complete about invariants, ordering, failure modes, and important
  performance characteristics.
- Do not put issue numbers in code comments. Record durable reasoning in the comment or an ADR so it
  remains understandable without tracker history.
- Update or remove nearby comments and docs when behavior changes. Stale explanation is a defect.

## Completion and review

- Run focused tests while working, type-check regularly, and run the repository's relevant final
  checks before handoff. A focused test passing is not evidence that the suite passes.
- Review the finished diff against both axes: repository standards and the originating Spec. Passing
  one axis does not compensate for failing the other.
- Treat formatting and lint findings as tooling concerns. Human review spends judgment on behavior,
  domain language, authority, depth, seams, safety, and test quality.
- Keep exemptions narrow and explain why the rule does not fit. Delete an exemption when its reason no
  longer exists.
