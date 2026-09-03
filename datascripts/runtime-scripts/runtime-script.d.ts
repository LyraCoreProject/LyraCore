/// The Runtime Script Host API, as TypeScript sees it.
///
/// Ambient on purpose: the Host hands an Invocation globals, not a module, so an authored script
/// imports nothing and exports nothing. A file with an `import` or an `export` becomes a TypeScript
/// module, and `typescript-to-lua` then emits a chunk that returns its export table instead of the
/// Script Answer.
///
/// Hand-maintained against `module/src/runtime_script.rs`. The EVENT names are not here: they are a
/// closed catalogue the Module's own build owns, and `lyracore packages build` checks a script's
/// `@event` directive against `lyracore-delta-check --print-events` rather than against a copy that
/// can drift.
///
/// Everything a script may reach is below. A name outside this file is nil inside an Invocation.

/// One creature or player the Host resolved for this Invocation.
///
/// Opaque: a script cannot read a guid out of it and cannot mint one, so the only entities a script
/// can act on are the ones its event carried. Every field is a snapshot taken before the script
/// ran; writing to one changes nothing.
interface Entity {
  readonly name: string;
  readonly is_player: boolean;
  readonly level: number;
  readonly health: number;
  readonly max_health: number;
  readonly map_id: number;
  readonly x: number;
  readonly y: number;
  readonly z: number;
}

/// What this Invocation is running for.
interface ScriptEvent {
  /// The event label, as the Script Artifact spells it.
  readonly name: string;
  /// Who caused the event, when the event carries one.
  readonly actor?: Entity;
  /// What the event acted on, when the event carries one.
  readonly target?: Entity;
}

declare const event: ScriptEvent;

/// Stage a heal on `entity`, crediting heal-threat to `event.actor`.
declare function heal(entity: Entity, amount: number): void;

/// Stage a System Message to one online player.
declare function send_chat(player: Entity, text: string): void;

/// Stage an experience grant.
declare function grant_xp(player: Entity, amount: number): void;

// Every authored `.ts` Runtime Script declares its own entry point:
//
//     function script(): number | void { … }
//
// `lyracore packages build` appends `return script()` to the emitted Lua, so the number it returns
// is the Script Answer the asking Package reads back. Returning nothing answers nothing, and the
// caller keeps its own fallback. The build refuses a script file that declares no `script`.
//
// It is not declared here: an ambient declaration and the author's own implementation would be two
// declarations of one name.
