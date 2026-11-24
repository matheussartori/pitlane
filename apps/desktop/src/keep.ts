// Keeping what did not change across a poll, so what is drawn from it is not redrawn.

/** A fresh list (cards, runs), reusing each old object whose content did not change: what is
 *  drawn from an unchanged one keeps its DOM, its focus and its hover across a poll. */
export function keepUnchanged<T extends { id: number | string }>(old: T[] | null, fresh: T[]): T[];
/** The same for a list whose items are told apart by something other than `id` (a lane's branch). */
export function keepUnchanged<T>(old: T[] | null, fresh: T[], key: (x: T) => number | string): T[];
export function keepUnchanged<T>(old: T[] | null, fresh: T[], key: (x: T) => number | string = (x) => (x as { id: number | string }).id): T[] {
  if (!old) return fresh;
  const before = new Map(old.map((c) => [key(c), c]));
  const next = fresh.map((c) => {
    const o = before.get(key(c));
    return o && JSON.stringify(o) === JSON.stringify(c) ? o : c;
  });
  return next.length === old.length && next.every((c, i) => c === old[i]) ? old : next;
}
