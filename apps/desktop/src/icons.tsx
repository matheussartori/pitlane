// Icons from design/index.html (24px grid, 1.6 stroke), plus the few the app adds.
import type { JSX } from "solid-js";

const paths: Record<string, () => JSX.Element> = {
  mark: () => (<><path d="M4 20V4" /><path d="M4 5h12l-2.5 4L16 13H4" /><path d="M14 20h6" /></>),
  needs: () => (<><path d="M5 21V4" /><path d="M5 4h11l-2 4 2 4H5" /></>),
  run: () => (<><path d="M21 12a9 9 0 1 1-9-9" /><path d="M12 7v5l3 2" /></>),
  verify: () => (<><path d="M12 3l7 3v6c0 4.5-3 7.5-7 9-4-1.5-7-4.5-7-9V6z" /><path d="M9 12l2 2 4-4" /></>),
  review: () => (<><path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12z" /><circle cx="12" cy="12" r="3" /></>),
  done: () => <path d="M5 12.5l4.5 4.5L19 7.5" />,
  fail: () => (<><circle cx="12" cy="12" r="9" /><path d="M9 9l6 6M15 9l-6 6" /></>),
  blocked: () => (<><rect x="5" y="11" width="14" height="10" rx="2" /><path d="M8 11V8a4 4 0 0 1 8 0v3" /></>),
  queued: () => (<><circle cx="12" cy="12" r="9" /><path d="M12 7v5h4" /></>),
  branch: () => (<><circle cx="6" cy="5" r="2" /><circle cx="6" cy="19" r="2" /><circle cx="18" cy="8" r="2" /><path d="M6 7v10M18 10c0 4-6 3-12 7" /></>),
  term: () => <path d="M5 8l4 4-4 4M12 17h7" />,
  search: () => (<><circle cx="11" cy="11" r="6" /><path d="M20 20l-4.5-4.5" /></>),
  chev: () => <path d="M7 10l5 5 5-5" />,
  plus: () => <path d="M12 5v14M5 12h14" />,
  trash: () => <path d="M4 7h16M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3" />,
  refresh: () => (<><path d="M20 11a8 8 0 1 0-2.3 5.7" /><path d="M20 4v7h-7" /></>),
  ship: () => (<><path d="M5 12h12" /><path d="M13 7l5 5-5 5" /></>),
  undo: () => (<><path d="M9 14L4 9l5-5" /><path d="M4 9h10a6 6 0 0 1 0 12h-3" /></>),
  person: () => (<><circle cx="12" cy="8" r="4" /><path d="M4 20c1.5-4 4.5-6 8-6s6.5 2 8 6" /></>),
  close: () => <path d="M6 6l12 12M18 6L6 18" />,
  copy: () => (<><rect x="8" y="8" width="12" height="12" rx="2" /><path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2" /></>),
  zoom: () => <path d="M14 4h6v6M10 20H4v-6M20 4l-6 6M4 20l6-6" />,
  unzoom: () => <path d="M20 10h-6V4M4 14h6v6M14 10l6-6M10 14l-6 6" />,
  up: () => <path d="M7 14l5-5 5 5" />,
  "tabs-top": () => (<><rect x="3" y="4" width="18" height="16" rx="2" /><path d="M3 9h18" /></>),
  "tabs-side": () => (<><rect x="3" y="4" width="18" height="16" rx="2" /><path d="M9 4v16" /></>),
  folder: () => <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />,
};

export type IconName = keyof typeof paths;

export function Icon(props: { name: IconName; small?: boolean; class?: string }) {
  return (
    <svg
      class={`icon${props.small ? " icon-sm" : ""}${props.class ? ` ${props.class}` : ""}`}
      viewBox="0 0 24 24"
      aria-hidden="true"
    >
      {paths[props.name]!()}
    </svg>
  );
}
