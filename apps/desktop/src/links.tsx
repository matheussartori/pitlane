// Web links inside plain text: styled as links, opened in the browser, never followed in the app.
import { For, type JSX } from "solid-js";
import { openExternal } from "./api";

const URL_RE = /(https?:\/\/[^\s<>"'`]+[^\s<>"'`.,;:!?)\]}])/g;

/** Text with every http(s) URL as a link that opens in the browser. */
export function Linked(props: { text: string }): JSX.Element {
  return (
    <For each={props.text.split(URL_RE)}>
      {(part, i) =>
        i() % 2 === 1 ? (
          <a
            class="link"
            href={part}
            title={part}
            onClick={(e) => {
              e.preventDefault();
              e.stopPropagation();
              void openExternal(part);
            }}
            onPointerDown={(e) => e.stopPropagation()}
          >
            {part}
          </a>
        ) : (
          part
        )
      }
    </For>
  );
}
