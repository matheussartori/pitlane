// Small formatters shared by every screen. Text meant for people goes through i18n where it is
// used; these only turn values into strings.

/** What an error says: the backend's commands reject with plain strings. */
export const msg = (e: unknown): string => (typeof e === "string" ? e : e instanceof Error ? e.message : String(e));

/** 95 → 1:35, 4000 → 1:06:40. */
export function clock(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const ss = String(s % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${ss}` : `${m}:${ss}`;
}

/** A CSS colour (oklch included) as #rrggbb, through a 1px canvas. */
export function cssHex(css: string): string {
  const c = document.createElement("canvas");
  c.width = c.height = 1;
  const ctx = c.getContext("2d");
  if (!ctx) return "#000000";
  ctx.fillStyle = "#000";
  ctx.fillStyle = css;
  ctx.fillRect(0, 0, 1, 1);
  const [r, g, b] = ctx.getImageData(0, 0, 1, 1).data;
  return `#${[r, g, b].map((v) => v!.toString(16).padStart(2, "0")).join("")}`;
}
