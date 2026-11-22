// Pitlane's own words in the language chosen in Settings (English is the source).
// Each string is its own key: t("Not saved"). Translations live in locales/pt/*.ts, one file per
// screen, merged here. A string without a translation shows in English.
import { prefs } from "./prefs";

export type Lang = "en" | "pt-BR";

const PT: Record<string, string> = Object.assign(
  {},
  ...Object.values(import.meta.glob<{ default: Record<string, string> }>("./locales/pt/*.ts", { eager: true })).map((m) => m.default),
);

/** The interface's language: the one chosen, or this Mac's. */
export function lang(): Lang {
  const chosen = prefs().ui_language;
  const tag = !chosen || chosen === "system" ? navigator.language : chosen;
  return tag.toLowerCase().startsWith("pt") ? "pt-BR" : "en";
}

/** A string in the interface's language; {name} placeholders are filled from vars. */
export function t(en: string, vars?: Record<string, string | number>): string {
  const s = lang() === "pt-BR" ? (PT[en] ?? en) : en;
  return vars ? s.replace(/\{(\w+)\}/g, (m, k: string) => (k in vars ? String(vars[k]) : m)) : s;
}

/** A word that reads differently by place: its key is "English@context" (e.g. "Run@kind"),
 *  falling back to the plain English key. */
export function tx(en: string, context: string, vars?: Record<string, string | number>): string {
  const k = `${en}@${context}`;
  return lang() === "pt-BR" && k in PT ? t(k, vars) : t(en, vars);
}

/** One or many: plural(n, "{n} card", "{n} cards"), both translated, {n} filled. */
export function plural(n: number, one: string, other: string, vars?: Record<string, string | number>): string {
  return t(n === 1 ? one : other, { n, ...vars });
}

/** The locale for dates and numbers. */
export const locale = () => (lang() === "pt-BR" ? "pt-BR" : "en-US");
