// Icons from design/index.html (24px grid, 1.6 stroke), plus the few the app adds.
import type { JSX } from "solid-js";

const paths: Record<string, () => JSX.Element> = {
  // Pitlane: the straight and the pit lane leaving and rejoining it (a P), the car in the pit.
  mark: () => (<><path d="M7 9c0-3 2-5 5-5 3.4 0 6 2.2 6 5s-2.6 5-6 5c-3 0-5 1.5-5 4.5" /><path d="M7 3v18" /><circle cx="18" cy="9" r="1.8" fill="currentColor" stroke="none" /></>),
  needs: () => (<><path d="M5 21V4" /><path d="M5 4h11l-2 4 2 4H5" /></>),
  run: () => (<><path d="M21 12a9 9 0 1 1-9-9" /><path d="M12 7v5l3 2" /></>),
  mic: () => (<><rect x="9" y="3" width="6" height="11" rx="3" /><path d="M5.5 11a6.5 6.5 0 0 0 13 0M12 17.5V21" /></>),
  stop: () => <rect x="6.5" y="6.5" width="11" height="11" rx="1.5" />,
  play: () => <path d="M7 5.5v13a.5.5 0 0 0 .77.42l10.2-6.5a.5.5 0 0 0 0-.84L7.77 5.08A.5.5 0 0 0 7 5.5z" />,
  verify: () => (<><path d="M12 3l7 3v6c0 4.5-3 7.5-7 9-4-1.5-7-4.5-7-9V6z" /><path d="M9 12l2 2 4-4" /></>),
  review: () => (<><path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12z" /><circle cx="12" cy="12" r="3" /></>),
  done: () => <path d="M5 12.5l4.5 4.5L19 7.5" />,
  fail: () => (<><circle cx="12" cy="12" r="9" /><path d="M9 9l6 6M15 9l-6 6" /></>),
  blocked: () => (<><rect x="5" y="11" width="14" height="10" rx="2" /><path d="M8 11V8a4 4 0 0 1 8 0v3" /></>),
  queued: () => (<><circle cx="12" cy="12" r="9" /><path d="M12 7v5h4" /></>),
  branch: () => (<><circle cx="6" cy="5" r="2" /><circle cx="6" cy="19" r="2" /><circle cx="18" cy="8" r="2" /><path d="M6 7v10M18 10c0 4-6 3-12 7" /></>),
  term: () => <path d="M5 8l4 4-4 4M12 17h7" />,
  tower: () => (<><rect x="3.5" y="4.5" width="17" height="15" rx="2" /><path d="M14.5 4.5v15" /></>),
  search: () => (<><circle cx="11" cy="11" r="6" /><path d="M20 20l-4.5-4.5" /></>),
  chev: () => <path d="M7 10l5 5 5-5" />,
  plus: () => <path d="M12 5v14M5 12h14" />,
  edit: () => <path d="M4 20h4L19 9a2.8 2.8 0 0 0-4-4L4 16v4zM13.5 6.5l4 4" />,
  trash: () => <path d="M4 7h16M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3" />,
  refresh: () => (<><path d="M20 11a8 8 0 1 0-2.3 5.7" /><path d="M20 4v7h-7" /></>),
  ship: () => (<><path d="M5 12h12" /><path d="M13 7l5 5-5 5" /></>),
  undo: () => (<><path d="M9 14L4 9l5-5" /><path d="M4 9h10a6 6 0 0 1 0 12h-3" /></>),
  note: () => (<><path d="M5 5h14a1 1 0 0 1 1 1v10a1 1 0 0 1-1 1h-8l-4 3v-3H5a1 1 0 0 1-1-1V6a1 1 0 0 1 1-1z" /><path d="M8 9.5h8M8 12.5h5" /></>),
  spark: () => (<><path d="M12 3v4M12 17v4M3 12h4M17 12h4" /><path d="M12 8.5l1.2 2.3 2.3 1.2-2.3 1.2L12 15.5l-1.2-2.3L8.5 12l2.3-1.2z" /></>),
  person: () => (<><circle cx="12" cy="8" r="4" /><path d="M4 20c1.5-4 4.5-6 8-6s6.5 2 8 6" /></>),
  close: () => <path d="M6 6l12 12M18 6L6 18" />,
  copy: () => (<><rect x="8" y="8" width="12" height="12" rx="2" /><path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2" /></>),
  zoom: () => <path d="M14 4h6v6M10 20H4v-6M20 4l-6 6M4 20l6-6" />,
  unzoom: () => <path d="M20 10h-6V4M4 14h6v6M14 10l6-6M10 14l-6 6" />,
  up: () => <path d="M7 14l5-5 5 5" />,
  "tabs-top": () => (<><rect x="3" y="4" width="18" height="16" rx="2" /><path d="M3 9h18" /></>),
  "tabs-side": () => (<><rect x="3" y="4" width="18" height="16" rx="2" /><path d="M9 4v16" /></>),
  // The side list folding away and coming back (a panel with its edge and an arrow).
  "side-close": () => (<><rect x="3.5" y="4.5" width="17" height="15" rx="2" /><path d="M9 4.5v15M16 9.5L13.5 12l2.5 2.5" /></>),
  "side-open": () => (<><rect x="3.5" y="4.5" width="17" height="15" rx="2" /><path d="M9 4.5v15M13.5 9.5L16 12l-2.5 2.5" /></>),
  code: () => <path d="M9 7l-5 5 5 5M15 7l5 5-5 5" />,
  inbox: () => (<><path d="M4 13l2.4-7.2A1 1 0 0 1 7.35 5h9.3a1 1 0 0 1 .95.8L20 13v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1z" /><path d="M4 13h4.5l1 2h5l1-2H20" /></>),
  board: () => (<><rect x="3.5" y="4.5" width="17" height="15" rx="2" /><path d="M9.5 4.5v15M14.5 4.5v15" /></>),
  lanes: () => (<><path d="M4 7h16M4 12h16M4 17h10" /></>),
  ledger: () => (<><path d="M6.5 4.5h11a1 1 0 0 1 1 1v14h-12a2 2 0 0 1-2-2v-11a2 2 0 0 1 2-2z" /><path d="M4.5 17.5a2 2 0 0 1 2-2h12M9 9h6" /></>),
  gauge: () => (<><path d="M4.5 17a8 8 0 1 1 15 0" /><path d="M12 13l3.5-4" /><circle cx="12" cy="13" r="1.2" /></>),
  pause: () => <path d="M9 6.5v11M15 6.5v11" />,
  settings: () => (<><path d="M4 7h9M17 7h3M4 17h3M11 17h9" /><circle cx="15" cy="7" r="2" /><circle cx="9" cy="17" r="2" /></>),
  folder: () => <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />,
  preview: () => (<><rect x="3.5" y="4.5" width="17" height="15" rx="2" /><path d="M3.5 9h17M8 14l2 2 4-4" /></>),
  external: () => <path d="M14 4.5h5.5V10M19.5 4.5l-8 8M17 14v4.5a1 1 0 0 1-1 1H5.5a1 1 0 0 1-1-1V8a1 1 0 0 1 1-1H10" />,
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
