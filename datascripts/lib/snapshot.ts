// Reading the Base Snapshot: the derived `game_spell` / `game_spell_effect` rows the importer wrote
// with `--spell-snapshot`, which are the only base data a Datascript can see.
//
// The snapshot is READ-ONLY in the strong sense: this module returns copies, and nothing in the
// library writes to it. It is also the ONLY source of rows a Datascript can read. A Package's
// generated artifacts are never opened here, so one Package cannot observe another's Delta — not by
// policy, but because there is no code path that would.
//
// Column names in the snapshot are the DURABLE ones, the same names the artifact carries. The
// generated names an author writes are resolved to them by the catalogue in `schema.ts`.

import { MAX_SPELL_EFFECT_INDEX } from "./ids.ts";
import { BaseDataError } from "./refusals.ts";
import {
  SPELL_COLUMNS,
  SPELL_EFFECT_COLUMNS,
  type Column,
} from "./schema.ts";
import type { FieldValue } from "./canonical.ts";

/// The snapshot format this library reads.
const SNAPSHOT_VERSION = 1;

/// One base row: every claimable column, by its durable name.
export type BaseRow = ReadonlyMap<string, FieldValue>;

/// The base data one Datascript run reads.
export class BaseSnapshot {
  private readonly spells: Map<number, BaseRow>;
  /// Keyed by spell, then by effect index, so a clone can copy exactly the slots the base fills.
  private readonly effects: Map<number, Map<number, BaseRow>>;

  private constructor(
    spells: Map<number, BaseRow>,
    effects: Map<number, Map<number, BaseRow>>,
  ) {
    this.spells = spells;
    this.effects = effects;
  }

  /// Parse a snapshot. Refuses an unknown version and a row that does not carry every column,
  /// because a partial base row would silently become a partial clone.
  static parse(json: string, origin: string): BaseSnapshot {
    let root: unknown;
    try {
      root = JSON.parse(json);
    } catch (cause) {
      throw new BaseDataError(`${origin} is not valid JSON: ${cause}`);
    }
    if (typeof root !== "object" || root === null) {
      throw new BaseDataError(`${origin} is not a Base Snapshot`);
    }

    const record = root as Record<string, unknown>;
    if (record.version !== SNAPSHOT_VERSION) {
      throw new BaseDataError(
        `${origin} declares Base Snapshot version ${record.version}; this build reads version ${SNAPSHOT_VERSION}`,
      );
    }

    const spells = new Map<number, BaseRow>();
    for (const raw of expectArray(record.game_spell, origin, "game_spell")) {
      const spellId = expectNumber(raw, "spell_id", origin);
      spells.set(spellId, readRow(raw, SPELL_COLUMNS, origin, `spell ${spellId}`));
    }

    const effects = new Map<number, Map<number, BaseRow>>();
    for (const raw of expectArray(record.game_spell_effect, origin, "game_spell_effect")) {
      const spellId = expectNumber(raw, "spell_id", origin);
      const effectIndex = expectNumber(raw, "effect_index", origin);
      // A row outside the three real effect slots is one no Package could ever claim, so the
      // snapshot and the claim rules would disagree about what exists. Say so rather than hide it.
      if (effectIndex < 0 || effectIndex > MAX_SPELL_EFFECT_INDEX) {
        throw new BaseDataError(
          `${origin}: spell ${spellId} carries an effect at index ${effectIndex}; a spell has ` +
            `effects 0..=${MAX_SPELL_EFFECT_INDEX} and no Package can claim any other slot`,
        );
      }
      const slots = effects.get(spellId) ?? new Map<number, BaseRow>();
      slots.set(
        effectIndex,
        readRow(raw, SPELL_EFFECT_COLUMNS, origin, `spell ${spellId} effect ${effectIndex}`),
      );
      effects.set(spellId, slots);
    }

    return new BaseSnapshot(spells, effects);
  }

  /// The base header row, or a refusal naming the spell. A Datascript may only build on base data it
  /// can actually see.
  spell(spellId: number): BaseRow {
    const row = this.spells.get(spellId);
    if (!row) {
      throw new BaseDataError(
        `the Base Snapshot holds no spell ${spellId}. A Datascript reads base data only — it cannot ` +
          `see a row another Package invents, and it cannot clone a spell the import never loaded.`,
      );
    }
    return row;
  }

  /// The base effect rows of a spell, by effect index. Empty when the spell has none.
  effectsOf(spellId: number): ReadonlyMap<number, BaseRow> {
    return this.effects.get(spellId) ?? new Map();
  }

  /// How many rows the snapshot holds, for a build to report.
  get size(): { spells: number; effects: number } {
    let effects = 0;
    for (const slots of this.effects.values()) effects += slots.size;
    return { spells: this.spells.size, effects };
  }
}

function expectArray(
  value: unknown,
  origin: string,
  member: string,
): Record<string, unknown>[] {
  if (!Array.isArray(value)) {
    throw new BaseDataError(`${origin}: member \`${member}\` must be an array of rows`);
  }
  return value as Record<string, unknown>[];
}

function expectNumber(
  row: Record<string, unknown>,
  member: string,
  origin: string,
): number {
  const value = row[member];
  if (typeof value !== "number" || !Number.isInteger(value)) {
    throw new BaseDataError(`${origin}: row member \`${member}\` must be a whole number`);
  }
  return value;
}

/// One row's claimable columns, narrowed to the types the catalogue declares. The snapshot writer is
/// the Package Delta canonical writer, so an unsigned 64-bit value arrives as a decimal string here
/// exactly as it would inside a claim.
function readRow(
  raw: Record<string, unknown>,
  columns: Record<string, Column>,
  origin: string,
  where: string,
): BaseRow {
  const row = new Map<string, FieldValue>();
  for (const column of Object.values(columns)) {
    const value = raw[column.column];
    if (value === undefined) {
      throw new BaseDataError(
        `${origin}: ${where} has no column \`${column.column}\`; the snapshot does not match this build's schema`,
      );
    }
    row.set(column.column, readValue(column, value, origin, where));
  }
  return row;
}

function readValue(
  column: Column,
  value: unknown,
  origin: string,
  where: string,
): FieldValue {
  const refuse = () =>
    new BaseDataError(
      `${origin}: ${where} column \`${column.column}\` is not a \`${column.type}\``,
    );

  switch (column.type) {
    case "u64": {
      if (typeof value !== "string" || !/^\d+$/.test(value)) throw refuse();
      return { type: "u64", value: BigInt(value) };
    }
    case "bool":
      if (typeof value !== "boolean") throw refuse();
      return { type: "bool", value };
    case "string":
      if (typeof value !== "string") throw refuse();
      return { type: "string", value };
    case "f32":
      if (typeof value !== "number" || !Number.isFinite(value)) throw refuse();
      return { type: "f32", value: Math.fround(value) };
    default:
      if (typeof value !== "number" || !Number.isInteger(value)) throw refuse();
      return { type: column.type, value };
  }
}
