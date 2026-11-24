// Syntax colour for one line of code in a diff: enough to tell a name from a value at a glance, as
// Claude Code's diffs read in Warp. One line at a time with no state between lines, so a block
// comment or a template string that runs over several lines only colours its first: a hint, never
// a parser. Unknown file types and very long lines stay plain.
import type { JSX } from "solid-js";

type Lang = "c" | "rust" | "hash" | "css" | "json";

const BY_EXT: Record<string, Lang> = {
  ts: "c", tsx: "c", js: "c", jsx: "c", mjs: "c", cjs: "c", go: "c", java: "c", kt: "c", swift: "c", c: "c", h: "c", cc: "c", cpp: "c", hpp: "c", cs: "c", php: "c", dart: "c", scala: "c",
  rs: "rust",
  py: "hash", rb: "hash", sh: "hash", bash: "hash", zsh: "hash", toml: "hash", yaml: "hash", yml: "hash",
  css: "css", scss: "css", less: "css",
  json: "json", jsonc: "json",
};

/** Past this many lines a diff stays plain: colour is not worth five times the nodes. */
const MAX_LINES = 4000;
const MAX_LINE = 400;

/** The language a path is coloured as, or undefined to leave it plain. */
export function langOf(path: string, lines = 0): Lang | undefined {
  if (lines > MAX_LINES) return undefined;
  const name = path.slice(path.lastIndexOf("/") + 1);
  const dot = name.lastIndexOf(".");
  return dot < 0 ? undefined : BY_EXT[name.slice(dot + 1).toLowerCase()];
}

const KEYWORDS = new Set(
  ("as async await break case catch class const continue def default defer delete do elif else enum export extends false finally fn for from func function go if impl " +
    "implements import in interface is let loop match mod mut new nil not null of or package private protected pub public return self static struct super switch then this " +
    "throw trait true try type typeof undefined unless use var void where while with yield None True False and del except lambda pass raise fi done esac local").split(" "),
);

const STR = `"(?:\\\\.|[^"\\\\])*"`;
const NUM = `\\b\\d[\\d_]*(?:\\.\\d+)?[a-zA-Z%]*`;
const WORD = `[A-Za-z_$][\\w$]*`;
// Groups, in order: 1 comment, 2 string, 3 number, 4 word (5 in CSS: a custom property).
const RULES: Record<Lang, RegExp> = {
  c: new RegExp(`(//.*$|/\\*.*?\\*/)|(${STR}|'(?:\\\\.|[^'\\\\])*'|\`(?:\\\\.|[^\`\\\\])*\`)|(${NUM})|(${WORD})`, "g"),
  // A lifetime ('a) is not a string: only a char literal closes within a few characters.
  rust: new RegExp(`(//.*$|/\\*.*?\\*/)|(${STR}|'(?:\\\\[^']{1,8}|[^'\\\\])')|(${NUM})|(${WORD})`, "g"),
  hash: new RegExp(`(#.*$)|(${STR}|'[^']*')|(${NUM})|(${WORD})`, "g"),
  css: new RegExp(`(/\\*.*?\\*/)|(${STR}|'[^']*')|(${NUM})|([A-Za-z-][\\w-]*)|(--[\\w-]+)`, "g"),
  json: new RegExp(`(//.*$)|(${STR})|(-?${NUM})|(${WORD})`, "g"),
};

type Kind = "k" | "s" | "n" | "f" | "c";

/** What a word is: a keyword, a call or a property name (the coloured ones), or plain text. */
function word(lang: Lang, w: string, text: string, end: number): Kind | undefined {
  const next = /^\s*(.)/.exec(text.slice(end, end + 8))?.[1];
  if (lang === "css") return next === ":" ? "f" : undefined;
  if (lang === "json") return w === "true" || w === "false" || w === "null" ? "k" : undefined;
  if (KEYWORDS.has(w)) return "k";
  return next === "(" ? "f" : undefined;
}

/** One line as plain text and coloured spans (the `.tok-*` classes of the design system). */
export function highlight(text: string, lang: Lang | undefined): JSX.Element {
  if (!lang || text.length > MAX_LINE) return text;
  const out: JSX.Element[] = [];
  const rule = RULES[lang];
  rule.lastIndex = 0;
  let at = 0;
  let plain = "";
  const push = (kind: Kind | undefined, s: string) => {
    if (!kind) {
      plain += s;
      return;
    }
    if (plain) out.push(plain);
    plain = "";
    out.push(<span class={`tok-${kind}`}>{s}</span>);
  };
  for (let m = rule.exec(text); m; m = rule.exec(text)) {
    if (m[0] === "") {
      rule.lastIndex++;
      continue;
    }
    plain += text.slice(at, m.index);
    at = m.index + m[0].length;
    // A # only opens a comment at the start of a line or after a space ($#, a URL's #anchor).
    if (m[1] && lang === "hash" && m.index > 0 && !/\s/.test(text.charAt(m.index - 1))) {
      plain += "#";
      at = m.index + 1;
      rule.lastIndex = at;
      continue;
    }
    if (m[1]) push("c", m[0]);
    else if (m[2]) push(lang === "json" && /^\s*:/.test(text.slice(at, at + 4)) ? "f" : "s", m[0]);
    else if (m[3]) push("n", m[0]);
    else if (m[5]) push("n", m[0]);
    else if (m[4]) push(word(lang, m[4], text, at), m[0]);
    else plain += m[0];
  }
  plain += text.slice(at);
  if (plain) out.push(plain);
  return out;
}
