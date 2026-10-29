/**
 * Instant type suggestions from the project symbol index, shown while the
 * language server is still thinking (kotlin-lsp easily takes a few hundred ms
 * per keystroke, IDEA answers from its own index right away). Picking one
 * inserts the name and adds the import the server would have added.
 */
import type { Completion } from "@codemirror/autocomplete";
import type { EditorView } from "@codemirror/view";
import { call, commands, type SymbolKind, type SymbolMatch } from "@/platform/ipc";

const LIMIT = 40;
const TYPE_KINDS: ReadonlySet<SymbolKind> = new Set(["class", "interface", "enum", "struct", "typeAlias"]);
/** Languages whose import syntax we can write, and the files they can import from. */
const IMPORTABLE: Record<string, readonly string[]> = {
  kotlin: [".kt", ".kts", ".java"],
  java: [".java", ".kt"],
};

export function indexCompletionLanguage(language: string): boolean {
  return language in IMPORTABLE;
}

/** Type symbols matching `word` that `language` can import. */
export async function indexTypeOptions(word: string, language: string): Promise<Completion[]> {
  const exts = IMPORTABLE[language];
  if (!exts || !word) return [];
  const matches = await call(commands.findSymbols(word, "types", LIMIT)).catch(() => [] as SymbolMatch[]);
  return matches
    .filter((m) => TYPE_KINDS.has(m.kind) && exts.some((e) => m.path.endsWith(e)))
    .map((m) => ({
      label: m.name,
      type: m.kind,
      detail: m.container ? ` (${m.container})` : undefined,
      apply: (view: EditorView, _c: Completion, from: number, to: number) => {
        view.dispatch({
          changes: { from, to, insert: m.name },
          selection: { anchor: from + m.name.length },
          userEvent: "input.complete",
        });
        void addImport(view, m, language);
      },
    }));
}

const PACKAGE = /^\s*package\s+([\w.]+)/m;

/** Where a new import line goes and what it looks like; null when none is needed. */
export function importEdit(text: string, fqn: string, language: string): { at: number; insert: string } | null {
  const pkg = fqn.slice(0, fqn.lastIndexOf("."));
  if (PACKAGE.exec(text)?.[1] === pkg) return null;
  const esc = (s: string) => s.replace(/[.$]/g, "\\$&");
  if (new RegExp(`^\\s*import\\s+(?:${esc(fqn)}|${esc(pkg)}\\.\\*)\\s*;?\\s*$`, "m").test(text)) return null;
  const line = language === "java" ? `import ${fqn};` : `import ${fqn}`;
  const imports = [...text.matchAll(/^[ \t]*import\s+[^\n]*$/gm)];
  const last = imports[imports.length - 1];
  if (last) return { at: last.index + last[0].length, insert: `\n${line}` };
  const pkgLine = /^[ \t]*package\s+[^\n]*$/m.exec(text);
  if (pkgLine) return { at: pkgLine.index + pkgLine[0].length, insert: `\n\n${line}` };
  return { at: 0, insert: `${line}\n\n` };
}

async function addImport(view: EditorView, m: SymbolMatch, language: string) {
  // hmm, the index has no package for top level symbols, so read it from the declaring file
  const source = await call(commands.fsReadText(m.path)).catch(() => null);
  const pkg = source && PACKAGE.exec(source)?.[1];
  if (!pkg || !view.dom.isConnected) return;
  const fqn = [pkg, m.container, m.name].filter(Boolean).join(".");
  const edit = importEdit(view.state.doc.toString(), fqn, language);
  if (edit) view.dispatch({ changes: { from: edit.at, insert: edit.insert }, userEvent: "input.complete.import" });
}
