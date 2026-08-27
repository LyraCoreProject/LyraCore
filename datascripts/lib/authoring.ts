// The typed surface a Datascript writes against: `data.spell(id)`, `.clone(newId)`, `.set(field,
// value)`, `.effect(0 | 1 | 2)`.
//
// # What an assignment means
//
// A `set` records ONE claim on ONE column, nothing else. A spell an author reads but never assigns
// produces no claim at all, so an unassigned field of an existing row keeps whatever the base import
// gives it — the Package states what it changes and stays silent about the rest.
//
// A CLONE is the other shape: the new row does not exist, so its claim carries the whole row, copied
// from the source and then overwritten by whatever the author assigns. Its child effects are cloned
// and claimed with it, at the same effect indexes, under the packed identifier rule. Cloning is the
// only way to invent a spell, which is why an insert can never be incomplete.
//
// # One handle per row
//
// `data.spell(id)` returns the SAME object every time it is called for one identifier, so two
// helpers tuning the same spell merge into one claim instead of colliding. Two clones onto one
// identifier do collide, and are refused.

import type { Spell, SpellEffect } from "../generated/types";
import type { FieldValue, PrimaryKey } from "./canonical.ts";
import {
  buildClaim,
  narrowValue,
  resolveColumn,
  spellEffectKey,
  spellKey,
} from "./delta.ts";
import { isPackageSpellId } from "./ids.ts";
import { BaseDataError, duplicateClaim, spellIdNotClientSafe } from "./refusals.ts";
import { BaseSnapshot, type BaseRow } from "./snapshot.ts";
import {
  SPELL_EFFECT_TABLE,
  SPELL_TABLE,
  type SpellEffectField,
  type SpellField,
} from "./schema.ts";
import { describeKey, type Claim } from "./canonical.ts";

/// The effect slots a spell has.
export type EffectIndex = 0 | 1 | 2;

/// One row an author is building: the claim shape plus the base values a `get` falls back to.
abstract class RowAuthor {
  protected readonly claimed = new Map<string, FieldValue>();

  protected constructor(
    readonly key: PrimaryKey,
    readonly operation: "insert" | "update",
    protected readonly base: BaseRow,
    protected readonly table: string,
  ) {}

  /// Record a claim on one column.
  protected assign(field: string, value: unknown): void {
    const column = resolveColumn(this.table, field);
    this.claimed.set(column.column, narrowValue(column, value));
  }

  /// The value this row will hold: what the author assigned, or the base value they did not.
  protected read(field: string): unknown {
    const column = resolveColumn(this.table, field);
    const value = this.claimed.get(column.column) ?? this.base.get(column.column);
    if (!value) {
      throw new BaseDataError(`no base value for \`${column.column}\``);
    }
    return value.value;
  }

  /// The row as it now stands: the base values with the author's assignments over them. This is
  /// what a clone copies, so an author who tunes a spell and then clones it gets the tuned values —
  /// which is what they are looking at.
  rowAsClaimed(): BaseRow {
    const row = new Map(this.base);
    for (const [name, value] of this.claimed) row.set(name, value);
    return row;
  }

  /// The claim this row contributes, or nothing when an existing row was read but never assigned.
  claim(): Claim | undefined {
    if (this.operation === "update") {
      if (this.claimed.size === 0) return undefined;
      return buildClaim(this.key, "update", new Map(this.claimed));
    }
    // An insert carries the whole row: the cloned base values, then the author's assignments.
    return buildClaim(this.key, "insert", new Map(this.rowAsClaimed()));
  }
}

/// One `game_spell_effect` row.
export class SpellEffectAuthor extends RowAuthor {
  constructor(key: PrimaryKey, operation: "insert" | "update", base: BaseRow) {
    super(key, operation, base, SPELL_EFFECT_TABLE);
  }

  set<K extends SpellEffectField>(field: K, value: SpellEffect[K]): this {
    this.assign(field, value);
    return this;
  }

  get<K extends SpellEffectField>(field: K): SpellEffect[K] {
    return this.read(field) as SpellEffect[K];
  }
}

/// One `game_spell` row and its effects.
export class SpellAuthor extends RowAuthor {
  private readonly effects = new Map<number, SpellEffectAuthor>();

  constructor(
    private readonly registry: ClaimRegistry,
    key: PrimaryKey,
    operation: "insert" | "update",
    base: BaseRow,
    baseEffects: ReadonlyMap<number, BaseRow>,
  ) {
    super(key, operation, base, SPELL_TABLE);
    for (const [index, row] of baseEffects) {
      this.effects.set(
        index,
        new SpellEffectAuthor(spellEffectKey(key.spellId, index), operation, row),
      );
    }
  }

  set<K extends SpellField>(field: K, value: Spell[K]): this {
    this.assign(field, value);
    return this;
  }

  get<K extends SpellField>(field: K): Spell[K] {
    return this.read(field) as Spell[K];
  }

  /// One of this spell's effects. A slot the base row does not fill has no row to claim: an insert
  /// there would have to invent every column, and an update would target a row that does not exist.
  effect(index: EffectIndex): SpellEffectAuthor {
    spellEffectKey(this.key.spellId, index); // the index policy, before the base-data question
    const effect = this.effects.get(index);
    if (!effect) {
      throw new BaseDataError(
        `spell ${this.key.spellId} has no effect ${index} in the Base Snapshot`,
      );
    }
    return effect;
  }

  /// A new spell at `newId`, carrying this spell's whole row and every effect it has. The new
  /// identifier must sit in the Package Spell Range; the copy is a starting point an author then
  /// assigns over.
  clone(newId: number): SpellAuthor {
    const effects = new Map<number, BaseRow>();
    for (const [index, effect] of this.effects) {
      effects.set(index, effect.rowAsClaimed());
    }
    return this.registry.insert(newId, this.rowAsClaimed(), effects);
  }

  /// Every claim this spell and its effects contribute.
  claims(): Claim[] {
    const claims: Claim[] = [];
    const own = this.claim();
    if (own) claims.push(own);
    for (const effect of this.effects.values()) {
      const claim = effect.claim();
      if (claim) claims.push(claim);
    }
    return claims;
  }
}

/// Every row one Datascript run claims. It hands out one author per identifier, which is what makes
/// two claims on one row impossible to write by accident and a second clone onto one identifier a
/// refusal.
export class ClaimRegistry {
  private readonly spells = new Map<number, SpellAuthor>();

  constructor(private readonly snapshot: BaseSnapshot) {}

  /// The author for an existing spell, reading the Base Snapshot.
  spell(spellId: number): SpellAuthor {
    const key = spellKey(spellId);
    const existing = this.spells.get(spellId);
    if (existing) return existing;

    const author = new SpellAuthor(
      this,
      key,
      "update",
      this.snapshot.spell(spellId),
      this.snapshot.effectsOf(spellId),
    );
    this.spells.set(spellId, author);
    return author;
  }

  /// The author for a spell this Package invents.
  insert(
    spellId: number,
    base: BaseRow,
    baseEffects: Map<number, BaseRow>,
  ): SpellAuthor {
    const key = spellKey(spellId);
    // The same rule `buildClaim` applies to a finished insert, applied at the clone instead, so the
    // refusal names the line that chose the identifier rather than the emit at the end of the run.
    if (!isPackageSpellId(spellId)) throw spellIdNotClientSafe(spellId);
    if (this.spells.has(spellId)) {
      throw duplicateClaim(SPELL_TABLE, describeKey(key));
    }
    const author = new SpellAuthor(this, key, "insert", base, baseEffects);
    this.spells.set(spellId, author);
    return author;
  }

  /// Every claim the run produced.
  claims(): Claim[] {
    return [...this.spells.values()].flatMap((author) => author.claims());
  }
}
