// The two things this printer owns: the shape generated Lua must not take on the pinned
// interpreter, and the entry point every Runtime Script declares.
//
// # The table-constructor guard
//
// piccolo 0.3.3 passes an inline table constructor's ELEMENT COUNT as an extra argument when the
// constructor is the LAST argument of a call, so `f({7, 8, 9})` reaches a two-parameter `f` as
// `f(table, 3)`. `x = x or default` is the commonest shape in transpiler output, so generated Lua
// meets it constantly. Both facts are pinned in the Module by
// `piccolo_leaks_a_table_constructors_element_count_as_an_extra_argument`.
//
// Parentheses do not help: the leak is in how the call site counts arguments, not in value
// adjustment. Passing the table through a one-parameter function does, because that function drops
// the extra value and returns exactly one. So this printer rewrites `f(a, {…})` to
// `f(a, ____tbl({…}))` and prepends `____tbl` to every emitted file.
//
// A printer rather than a TypeScript visitor: it sees the whole Lua AST, including the calls tstl
// itself invents for classes, spreads and library helpers, which a visitor over TypeScript nodes
// never meets.
//
// Named Event Bindings get their invocation wrapper from build-scripts.ts. Legacy sources still
// declare script(), whose return value becomes the Script Answer through the appended call.

const ts = require("typescript");
const tstl = require("typescript-to-lua");
const lua = require("typescript-to-lua/dist/LuaAST");

/// The one-parameter passthrough. Named in the `____` space tstl reserves for its own locals.
const GUARD = "____tbl";

/// The function name every authored `.ts` Runtime Script declares.
const ENTRY = "script";

/// `params` with a trailing table constructor wrapped in the guard. Only the last argument is
/// rewritten: Lua adjusts only the last argument of a call to multiple values, and that is exactly
/// where the leak lands.
function guardTrailingTable(params) {
  if (!params || params.length === 0) return params;
  const last = params[params.length - 1];
  if (!lua.isTableExpression(last)) return params;
  const wrapped = lua.createCallExpression(lua.createIdentifier(GUARD), [last]);
  return [...params.slice(0, -1), wrapped];
}

/// `local function ____tbl(t) return t end`, prepended to every file whether or not a call needed
/// it. Emitting it unconditionally keeps the output a pure function of the source: whether the
/// guard is reachable never changes a byte anywhere else.
function guardDeclaration() {
  const parameter = lua.createIdentifier("t");
  return lua.createVariableDeclarationStatement(
    lua.createIdentifier(GUARD),
    lua.createFunctionExpression(
      lua.createBlock([lua.createReturnStatement([parameter])]),
      [parameter],
    ),
  );
}

/// `return script()` — the last statement of every emitted file, so the chunk's return value is the
/// Script Answer.
function entryCall() {
  return lua.createReturnStatement([
    lua.createCallExpression(lua.createIdentifier(ENTRY), []),
  ]);
}

class PiccoloPrinter extends tstl.LuaPrinter {
  printCallExpression(expression) {
    // The guard's own call must not be wrapped again, or printing never terminates.
    if (lua.isIdentifier(expression.expression) && expression.expression.text === GUARD) {
      return super.printCallExpression(expression);
    }
    return super.printCallExpression({ ...expression, params: guardTrailingTable(expression.params) });
  }

  printMethodCallExpression(expression) {
    return super.printMethodCallExpression({
      ...expression,
      params: guardTrailingTable(expression.params),
    });
  }

  printFile(file) {
    return super.printFile({
      ...file,
      statements: [guardDeclaration(), ...file.statements, ...(this.bound ? [] : [entryCall()])],
    });
  }
}

function hasEventBinding(file) {
  return file.statements.some((statement) => {
    if (!ts.isExpressionStatement(statement) || !ts.isCallExpression(statement.expression)) return false;
    let target = statement.expression.expression;
    while (ts.isPropertyAccessExpression(target)) target = target.expression;
    return ts.isIdentifier(target) && target.text === "events";
  });
}

function acceptsScriptAnswer(type) {
  if (type.isUnion()) return type.types.every(acceptsScriptAnswer);
  return (type.flags & (ts.TypeFlags.NumberLike | ts.TypeFlags.Void | ts.TypeFlags.Undefined | ts.TypeFlags.Never)) !== 0;
}

/// Every non-declaration source file must declare `function script(): number | void` with no
/// parameters. The Host invokes it without arguments and reads only a numeric Script Answer.
function requireEntryPoint(program) {
  const diagnostics = [];
  const checker = program.getTypeChecker();
  for (const file of program.getSourceFiles()) {
    if (file.isDeclarationFile || hasEventBinding(file)) continue;
    const declarations = file.statements.filter(
      (statement) =>
        ts.isFunctionDeclaration(statement) && statement.name && statement.name.text === ENTRY,
    );
    const implementations = declarations.filter((declaration) => declaration.body !== undefined);
    if (implementations.length !== 1) {
      diagnostics.push({
        file,
        start: declarations[0]?.getStart(file) ?? 0,
        length: declarations[0]?.getWidth(file) ?? 0,
        category: ts.DiagnosticCategory.Error,
        code: 0,
        messageText:
          `a Runtime Script declares exactly one concrete entry point as ` +
          `\`function ${ENTRY}(): number | void\`. ` +
          `The emitted Lua ends with \`return ${ENTRY}()\`, because TypeScript has no top-level ` +
          "return and a Script Answer is the chunk's return value.",
      });
      continue;
    }

    const declaration = implementations[0];
    const signature = checker.getSignatureFromDeclaration(declaration);
    const returnType = signature && checker.getReturnTypeOfSignature(signature);
    if (declaration.parameters.length === 0 && returnType && acceptsScriptAnswer(returnType)) {
      continue;
    }
    diagnostics.push({
      file,
      start: declaration.getStart(file),
      length: declaration.getWidth(file),
      category: ts.DiagnosticCategory.Error,
      code: 0,
      messageText:
        `\`${ENTRY}\` must take no parameters and return only a number or nothing. ` +
        "The Runtime Script Host calls it without arguments and accepts only a numeric Script Answer.",
    });
  }
  return diagnostics;
}

module.exports = {
  beforeTransform: (program) => requireEntryPoint(program),
  printer: (program, emitHost, fileName, file) => {
    const printer = new PiccoloPrinter(emitHost, program, fileName);
    printer.bound = program.getSourceFiles().some((source) => !source.isDeclarationFile && hasEventBinding(source));
    return printer.print(file);
  },
};
