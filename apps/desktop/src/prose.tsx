// Small markdown for specs and notes: paragraphs, bullets, headings as bold lines, fenced code,
// `code`, **bold** and web links. Never HTML.
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

/** A note's markdown, small: paragraphs, bullet lists, headings as bold lines, fenced code. */
export function Prose(props: { text: string }) {
  const blocks = () => {
    const out: ({ kind: "p" | "h"; text: string } | { kind: "ul"; items: string[] } | { kind: "pre"; text: string })[] = [];
    const lines = props.text.replace(/\r/g, "").split("\n");
    for (let i = 0; i < lines.length; i++) {
      const line = lines[i]!;
      if (line.startsWith("```")) {
        const code: string[] = [];
        while (++i < lines.length && !lines[i]!.startsWith("```")) code.push(lines[i]!);
        out.push({ kind: "pre", text: code.join("\n") });
      } else if (/^\s*[-*] /.test(line)) {
        const last = out[out.length - 1];
        const item = line.replace(/^\s*[-*] /, "");
        if (last?.kind === "ul") last.items.push(item);
        else out.push({ kind: "ul", items: [item] });
      } else if (/^#{1,6} /.test(line)) {
        out.push({ kind: "h", text: line.replace(/^#+ /, "") });
      } else if (line.trim() === "") {
        out.push({ kind: "p", text: "" });
      } else {
        const last = out[out.length - 1];
        if (last?.kind === "p" && last.text) last.text += ` ${line.trim()}`;
        else out.push({ kind: "p", text: line.trim() });
      }
    }
    return out.filter((b) => !(b.kind === "p" && !b.text));
  };
  return (
    <div class="prose">
      <For each={blocks()}>
        {(b) =>
          b.kind === "ul" ? (
            <ul>
              <For each={b.items}>{(x) => <li>{inline(x)}</li>}</For>
            </ul>
          ) : b.kind === "pre" ? (
            <pre>{b.text}</pre>
          ) : b.kind === "h" ? (
            <p class="prose-h">{inline(b.text)}</p>
          ) : (
            <p>{inline(b.text)}</p>
          )
        }
      </For>
    </div>
  );
}
