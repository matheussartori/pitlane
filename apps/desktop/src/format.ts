// Small formatters shared by every screen: values turned into the short strings the screens show.

import { isLive, type RunMeta } from "./api";
import { locale, t } from "./i18n";

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

/** How long ago, from a span in milliseconds: "just now", "12m ago", "3h ago", "2d ago". */
export function ago(ms: number): string {
  const s = Math.max(0, Math.round(ms / 1000));
  if (s < 45) return t("just now");
  const m = Math.round(s / 60);
  if (m < 60) return t("{n}m ago", { n: m });
  const h = Math.round(m / 60);
  if (h < 24) return t("{n}h ago", { n: h });
  return t("{n}d ago", { n: Math.round(h / 24) });
}

/** A note's age in words: "now", "12 min", "3 h", "2 d", then the date. */
export function age(at: string, now: number): string {
  const ts = Date.parse(at);
  if (Number.isNaN(ts)) return at;
  const m = Math.floor((now - ts) / 60000);
  if (m < 1) return t("now");
  if (m < 60) return t("{n} min ago", { n: m });
  if (m < 60 * 24) return t("{n} h ago", { n: Math.floor(m / 60) });
  if (m < 60 * 24 * 7) return t("{n} d ago", { n: Math.floor(m / 1440) });
  return new Date(ts).toLocaleDateString(locale(), { day: "numeric", month: "short" });
}

/** Wall time of a run: ticking while live, fixed once it ends. */
export function elapsed(m: RunMeta, nowMs: number): number {
  const end = m.ended_at ?? (isLive(m.state) ? nowMs / 1000 : m.started_at + (m.duration_ms ?? 0) / 1000);
  return end - m.started_at;
}

export const money = (n: number | null | undefined) => (n == null ? "" : `$${n.toFixed(2)}`);
export const kTokens = (n: number) =>
  n >= 1_000_000 ? `${(n / 1_000_000).toFixed(n >= 10_000_000 ? 0 : 1)}M` : n >= 1000 ? `${(n / 1000).toFixed(n >= 10_000 ? 0 : 1)}k` : String(n);
