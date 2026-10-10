import * as ts from "typescript";
import * as lua from "luaparse";
import { EVENTS, type EventDefinition } from "./generate-events.ts";

export interface SourceBinding {
  readonly event: string;
  readonly group: string;
  readonly method: string;
  readonly priority: number;
  readonly enabled: boolean;
  readonly definition?: EventDefinition;
}

function refuse(file: string, message: string): never {
  throw new Error(`${file}: ${message}`);
}

function binding(
  file: string, packageName: string, path: string[], localName: string | undefined,
  options: Record<string, unknown>,
): SourceBinding {
  if (path.length !== 3 || path[0] !== "events") refuse(file, "use events.<group>.<event>(handler)");
  const group = path[1]!;
  const method = path[2]!;
  const definition = EVENTS.find((event) => event.registration[0] === group && event.registration[1] === method);
  let event = definition?.event;
  if (group === "package" && method === "on") {
    if (!localName || !/^[a-z][a-z0-9_]*$/.test(localName)) {
      refuse(file, "a Package Event needs a literal local name such as \"greeting\"");
    }
    event = `${packageName}.${localName}`;
  }
  if (!event) refuse(file, `unknown Event Binding ${path.join(".")}`);
  for (const key of Object.keys(options)) {
    if (key !== "priority" && key !== "enabled") refuse(file, `unknown Event Binding option ${key}`);
  }
  const priority = options.priority ?? 0;
  const enabled = options.enabled ?? true;
  if (typeof priority !== "number" || !Number.isInteger(priority) || priority < -2147483648 || priority > 2147483647) {
    refuse(file, "priority must be a literal signed 32-bit integer");
  }
  if (typeof enabled !== "boolean") refuse(file, "enabled must be a literal boolean");
  return { event, group, method, priority, enabled, definition };
}

function tsPath(expression: ts.Expression): string[] {
  if (ts.isIdentifier(expression)) return [expression.text];
  if (ts.isPropertyAccessExpression(expression)) return [...tsPath(expression.expression), expression.name.text];
  return [];
}

function tsLiteral(file: string, expression: ts.Expression): unknown {
  if (expression.kind === ts.SyntaxKind.TrueKeyword) return true;
  if (expression.kind === ts.SyntaxKind.FalseKeyword) return false;
  if (ts.isNumericLiteral(expression)) return Number(expression.text);
  if (ts.isStringLiteral(expression)) return expression.text;
  if (ts.isPrefixUnaryExpression(expression) && ts.isNumericLiteral(expression.operand)) {
    if (expression.operator === ts.SyntaxKind.MinusToken) return -Number(expression.operand.text);
    if (expression.operator === ts.SyntaxKind.PlusToken) return Number(expression.operand.text);
  }
  return refuse(file, "Event Binding options must be literal values");
}

function tsBinding(file: string, source: string, packageName: string): SourceBinding | undefined {
  const tree = ts.createSourceFile(file, source, ts.ScriptTarget.Latest, true);
  const calls = tree.statements.filter(ts.isExpressionStatement)
    .map((statement) => statement.expression).filter(ts.isCallExpression)
    .filter((call) => tsPath(call.expression)[0] === "events");
  if (calls.length > 1) refuse(file, "each source file has exactly one Event Binding");
  const call = calls[0];
  let root: ts.Node | undefined = call?.expression;
  while (root && ts.isPropertyAccessExpression(root)) root = root.expression;
  const walk = (node: ts.Node): void => {
    if (ts.isIdentifier(node) && node.text === "events" && node !== root) {
      refuse(file, "events may only appear in one unconditional top-level Event Binding");
    }
    ts.forEachChild(node, walk);
  };
  walk(tree);
  if (!call) return undefined;
  if (ts.isExternalModule(tree)) refuse(file, "a Runtime Script cannot import or export a module");
  const path = tsPath(call.expression);
  const own = path[1] === "package" && path[2] === "on";
  const offset = own ? 1 : 0;
  const handler = call.arguments[offset];
  if (!handler || !ts.isIdentifier(handler)) refuse(file, "bind a named function declared in this file");
  const declarations = tree.statements.filter(ts.isFunctionDeclaration)
    .filter((fn) => fn.name?.text === handler.text && fn.body);
  if (declarations.length !== 1 || declarations[0]!.parameters.length !== 1) {
    refuse(file, "the bound function must declare one event parameter");
  }
  if (call.arguments.length < offset + 1 || call.arguments.length > offset + 2) {
    refuse(file, "an Event Binding takes a handler and optional options");
  }
  const options: Record<string, unknown> = {};
  const optionNode = call.arguments[offset + 1];
  if (optionNode) {
    if (!ts.isObjectLiteralExpression(optionNode)) refuse(file, "Event Binding options must be a literal object");
    for (const member of optionNode.properties) {
      if (!ts.isPropertyAssignment(member) || (!ts.isIdentifier(member.name) && !ts.isStringLiteral(member.name))) {
        refuse(file, "Event Binding options must be named literal properties");
      }
      const name = member.name.text;
      if (Object.hasOwn(options, name)) refuse(file, `duplicate Event Binding option ${name}`);
      options[name] = tsLiteral(file, member.initializer);
    }
  }
  const localName = own && call.arguments[0] && ts.isStringLiteral(call.arguments[0]) ? call.arguments[0].text : undefined;
  return binding(file, packageName, path, localName, options);
}

function luaPath(expression: lua.Expression): string[] {
  if (expression.type === "Identifier") return [expression.name];
  if (expression.type === "MemberExpression" && expression.indexer === ".") {
    return [...luaPath(expression.base), expression.identifier.name];
  }
  return [];
}

function luaLiteral(file: string, expression: lua.Expression): unknown {
  if (expression.type === "StringLiteral" || expression.type === "NumericLiteral" || expression.type === "BooleanLiteral") {
    return expression.value;
  }
  if (expression.type === "UnaryExpression" && expression.operator === "-" && expression.argument.type === "NumericLiteral") {
    return -expression.argument.value;
  }
  return refuse(file, "Event Binding options must be literal values");
}

function luaBinding(file: string, source: string, packageName: string): SourceBinding | undefined {
  let tree: lua.Chunk;
  try {
    tree = lua.parse(source, { luaVersion: "5.3", encodingMode: "x-user-defined" });
  } catch (error) {
    return refuse(file, `invalid Lua: ${error instanceof Error ? error.message : String(error)}`);
  }
  const calls = tree.body.filter((node): node is lua.CallStatement => node.type === "CallStatement")
    .map((node) => node.expression)
    .filter((node): node is lua.CallExpression => node.type === "CallExpression" && luaPath(node.base)[0] === "events");
  if (calls.length > 1) refuse(file, "each source file has exactly one Event Binding");
  const call = calls[0];
  let root = call?.base;
  while (root?.type === "MemberExpression") root = root.base;
  const walk = (node: unknown): void => {
    if (!node || typeof node !== "object") return;
    if (node === root) return;
    if ("type" in node && node.type === "Identifier" && "name" in node && node.name === "events") {
      refuse(file, "events may only appear in one unconditional top-level Event Binding");
    }
    for (const value of Object.values(node)) {
      if (Array.isArray(value)) value.forEach(walk);
      else walk(value);
    }
  };
  walk(tree);
  if (!call) return undefined;
  if (tree.body.some((node) => node.type === "ReturnStatement")) refuse(file, "return a Script Answer from the handler, not the source file");
  const path = luaPath(call.base);
  const own = path[1] === "package" && path[2] === "on";
  const offset = own ? 1 : 0;
  const handler = call.arguments[offset];
  if (!handler || handler.type !== "Identifier") refuse(file, "bind a named function declared in this file");
  const declarations = tree.body.filter((node): node is lua.FunctionDeclaration => node.type === "FunctionDeclaration")
    .filter((fn) => fn.identifier?.type === "Identifier" && fn.identifier.name === handler.name);
  if (declarations.length !== 1 || declarations[0]!.parameters.length !== 1 || declarations[0]!.parameters[0]?.type !== "Identifier") {
    refuse(file, "the bound function must declare one event parameter");
  }
  if (call.arguments.length < offset + 1 || call.arguments.length > offset + 2) {
    refuse(file, "an Event Binding takes a handler and optional options");
  }
  const options: Record<string, unknown> = {};
  const optionNode = call.arguments[offset + 1];
  if (optionNode) {
    if (optionNode.type !== "TableConstructorExpression") refuse(file, "Event Binding options must be a literal table");
    for (const field of optionNode.fields) {
      if (field.type !== "TableKeyString") refuse(file, "Event Binding options must be named literal fields");
      const name = field.key.name;
      if (Object.hasOwn(options, name)) refuse(file, `duplicate Event Binding option ${name}`);
      options[name] = luaLiteral(file, field.value);
    }
  }
  const localName = own && call.arguments[0]?.type === "StringLiteral" ? call.arguments[0].value ?? undefined : undefined;
  return binding(file, packageName, path, localName, options);
}

export function readBinding(file: string, source: string, packageName: string): SourceBinding | undefined {
  return file.endsWith(".ts") ? tsBinding(file, source, packageName) : luaBinding(file, source, packageName);
}

/** Capture the one declared handler inside the Invocation, then preserve its Script Answer. */
export function bindInvocation(source: string, binding: SourceBinding): string {
  let local = "____lyracore_handler";
  while (source.includes(local)) local += "_";
  const parameters = binding.group === "package" ? "name, handler" : "handler";
  const lines = [
    `local ${local}`,
    `local events = { ${binding.group} = { ${binding.method} = function(${parameters}) ${local} = handler end } }`,
    source.trimEnd(),
    `assert(event.name == ${JSON.stringify(binding.event)}, "Event Binding does not match the Invocation")`,
  ];
  for (const field of binding.definition?.fields ?? []) {
    if (field.type === "PlayerEntity" && field.required) {
      lines.push(`assert(event.${field.name} and event.${field.name}.is_player, "${binding.event} requires a Character")`);
    }
  }
  lines.push(`return ${local}(event)`, "");
  return lines.join("\n");
}
