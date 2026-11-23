// Small markdown for specs, notes and the agent's replies: paragraphs, lists, headings as bold
// lines, quotes, tables, fenced code, `code`, **bold** and web links. Never HTML.
import { For, type JSX } from "solid-js";
import { openExternal } from "./api";
import { Linked } from "./links";

const MD_LINK = /^\[([^\]]+)\]\((https?:\/\/[^)\s]+)\)$/;

/** Inline markdown: `code`, **bold**, [text](url) and bare URLs. Never HTML. */
export function inline(text: string): JSX.Element {
  return (
    <For each={text.split(/(`[^`]+`|\*\*[^*]+\*\*|\[[^\]]+\]\(https?:\/\/[^)\s]+\))/g).filter(Boolean)}>
      {(part) =>
        MD_LINK.test(part) ? (
          <a
            class="link"
            href={MD_LINK.exec(part)![2]}
            title={MD_LINK.exec(part)![2]}
            onClick={(e) => {
              e.preventDefault();
              e.stopPropagation();
              void openExternal(MD_LINK.exec(part)![2]!);
            }}
          >
            {MD_LINK.exec(part)![1]}
          </a>
        ) : part.startsWith("`") && part.endsWith("`") && part.length > 2 ? (
          <code>{part.slice(1, -1)}</code>
        ) : part.startsWith("**") && part.endsWith("**") && part.length > 4 ? (
          <b>{part.slice(2, -2)}</b>
        ) : (
          <Linked text={part} />
        )
      }
    </For>
  );
}

type Align = "left" | "center" | "right" | undefined;
type ProseBlock =
  | { kind: "p" | "h" | "quote"; text: string }
  | { kind: "ul"; items: string[] }
  | { kind: "ol"; start: number; items: string[] }
  | { kind: "pre"; text: string }
  | { kind: "hr" }
  | { kind: "table"; align: Align[]; head: string[]; rows: string[][] };

const TABLE_RULE = /^\s*\|?\s*:?-+:?\s*(\|\s*:?-+:?\s*)*\|?\s*$/;
const OL_ITEM = /^\s*(\d+)[.)] /;

/** A table row's cells: split on `|`, except an escaped `\|` or one inside `code`. */
function cells(line: string): string[] {
  const out: string[] = [];
  let cur = "";
  let code = false;
  const s = line.trim().replace(/^\|/, "").replace(/(?<!\\)\|$/, "");
  for (let i = 0; i < s.length; i++) {
    const c = s[i]!;
    if (c === "\\" && s[i + 1] === "|") {
      cur += "|";
      i++;
    } else if (c === "`") {
      code = !code;
      cur += c;
    } else if (c === "|" && !code) {
      out.push(cur.trim());
      cur = "";
    } else cur += c;
  }
  out.push(cur.trim());
  return out;
}

/** Splits markdown into the blocks `Prose` draws. */
export function proseBlocks(text: string): ProseBlock[] {
  const out: ProseBlock[] = [];
  const lines = text.replace(/\r/g, "").split("\n");
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i]!;
    const last = out[out.length - 1];
    if (line.startsWith("```")) {
      const code: string[] = [];
      while (++i < lines.length && !lines[i]!.startsWith("```")) code.push(lines[i]!);
      out.push({ kind: "pre", text: code.join("\n") });
    } else if (line.includes("|") && TABLE_RULE.test(lines[i + 1] ?? "") && cells(line).length === cells(lines[i + 1]!).length) {
      const head = cells(line);
      const align: Align[] = cells(lines[i + 1]!).map((r) =>
        r.startsWith(":") && r.endsWith(":") ? "center" : r.endsWith(":") ? "right" : r.startsWith(":") ? "left" : undefined,
      );
      const rows: string[][] = [];
      i++;
      while (i + 1 < lines.length && lines[i + 1]!.includes("|") && lines[i + 1]!.trim() !== "") {
        const row = cells(lines[++i]!);
        rows.push(head.map((_, k) => row[k] ?? ""));
      }
      out.push({ kind: "table", align, head, rows });
    } else if (/^\s*[-*] /.test(line)) {
      const item = line.replace(/^\s*[-*] /, "");
      if (last?.kind === "ul") last.items.push(item);
      else out.push({ kind: "ul", items: [item] });
    } else if (OL_ITEM.test(line)) {
      const item = line.replace(OL_ITEM, "");
      if (last?.kind === "ol") last.items.push(item);
      else out.push({ kind: "ol", start: Number(OL_ITEM.exec(line)![1]), items: [item] });
    } else if (/^\s*([-*_])(\s*\1){2,}\s*$/.test(line)) {
      out.push({ kind: "hr" });
    } else if (/^#{1,6} /.test(line)) {
      out.push({ kind: "h", text: line.replace(/^#+ /, "") });
    } else if (/^\s*>/.test(line)) {
      const quoted = line.replace(/^\s*> ?/, "").trim();
      if (last?.kind === "quote") last.text += last.text && quoted ? ` ${quoted}` : quoted;
      else out.push({ kind: "quote", text: quoted });
    } else if (line.trim() === "") {
      out.push({ kind: "p", text: "" });
    } else if ((last?.kind === "ul" || last?.kind === "ol") && /^\s{2,}\S/.test(line)) {
      // A list item's text going on over the next, indented, line.
      last.items[last.items.length - 1] += ` ${line.trim()}`;
    } else if (last?.kind === "p" && last.text) {
      last.text += ` ${line.trim()}`;
    } else out.push({ kind: "p", text: line.trim() });
  }
  return out.filter((b) => !((b.kind === "p" || b.kind === "quote") && !b.text));
}

/** A note's markdown, small: paragraphs, lists, headings as bold lines, quotes, tables, fenced code. */
export function Prose(props: { text: string }) {
  return (
    <div class="prose">
      <For each={proseBlocks(props.text)}>
        {(b) => {
          switch (b.kind) {
            case "ul":
              return (
                <ul>
                  <For each={b.items}>{(x) => <li>{inline(x)}</li>}</For>
                </ul>
              );
            case "ol":
              return (
                <ol start={b.start}>
                  <For each={b.items}>{(x) => <li>{inline(x)}</li>}</For>
                </ol>
              );
            case "pre":
              return <pre>{b.text}</pre>;
            case "hr":
              return <hr />;
            case "table":
              return (
                <div class="prose-table">
                  <table class="table">
                    <thead>
                      <tr>
                        <For each={b.head}>{(h, k) => <th scope="col" style={{ "text-align": b.align[k()] }}>{inline(h)}</th>}</For>
                      </tr>
                    </thead>
                    <tbody>
                      <For each={b.rows}>
                        {(row) => (
                          <tr>
                            <For each={row}>{(c, k) => <td style={{ "text-align": b.align[k()] }}>{inline(c)}</td>}</For>
                          </tr>
                        )}
                      </For>
                    </tbody>
                  </table>
                </div>
              );
            case "h":
              return <p class="prose-h">{inline(b.text)}</p>;
            case "quote":
              return <blockquote>{inline(b.text)}</blockquote>;
            default:
              return <p>{inline(b.text)}</p>;
          }
        }}
      </For>
    </div>
  );
}
