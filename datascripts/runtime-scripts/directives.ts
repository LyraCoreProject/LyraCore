/** Read legacy headers before the first line that is neither blank nor a comment. */
export function readDirectives(file: string, source: string): Map<string, string> {
  const directives = new Map<string, string>();
  for (const line of source.split("\n")) {
    const text = line.trim();
    if (text.length === 0) continue;
    const comment = text.startsWith("//") ? text.slice(2) : text.startsWith("--") ? text.slice(2) : undefined;
    if (comment === undefined) break;
    const match = /^\s*@([a-z]+)\s+(\S+)\s*$/.exec(comment);
    if (!match) continue;
    const [, key, value] = match as unknown as [string, string, string];
    if (directives.has(key)) throw new Error(`${file}: \`@${key}\` is declared twice`);
    directives.set(key, value);
  }
  return directives;
}
