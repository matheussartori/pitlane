// The app's own keys, on the window: the Tower, Settings, a fetch, a terminal, a new lane or card,
// and Option+number for the screens.
import type { Lane } from "./api";
import { ownsKey } from "./ui";
import { fetching, load, repo, yard } from "./store/repo";
import type { SheetState, Tab } from "./App";

/** What the keys reach in the shell. */
interface Shell {
  tab: () => Tab;
  setTab: (t: Tab) => void;
  sheet: () => SheetState;
  setSheet: (s: SheetState) => void;
  selected: () => string | null;
  setMenu: (open: boolean) => void;
  toggleTower: () => void;
  terminalIn: (lane: Lane | undefined) => Promise<void>;
}

/** The window's keydown listener; the shell adds it on mount and removes it on cleanup. */
export function shortcuts(shell: Shell): (e: KeyboardEvent) => void {
  const { tab, setTab, sheet, setSheet, selected, setMenu, toggleTower, terminalIn } = shell;

  const onKey = (e: KeyboardEvent) => {
    const typing = ownsKey(e);
    if (e.metaKey && e.key === "\\") {
      e.preventDefault();
      toggleTower();
    } else if (e.metaKey && e.key === ",") {
      e.preventDefault();
      setTab("settings");
    } else if (e.metaKey && e.key.toLowerCase() === "r") {
      e.preventDefault();
      if (!fetching()) void load(true);
    } else if (e.metaKey && !e.shiftKey && e.key.toLowerCase() === "t" && repo()) {
      // A terminal where you are: the selected lane's worktree on Lanes, else the repository.
      e.preventDefault();
      const lane = tab() === "lanes" ? yard()?.lanes.find((l) => l.branch === selected() && l.worktree) : undefined;
      void terminalIn(lane);
    } else if (!typing && !e.metaKey && !e.ctrlKey && !sheet() && repo()) {
      if ((e.key === "n" || e.key === "N") && tab() === "lanes") {
        e.preventDefault();
        setSheet({ kind: "new" });
      } else if ((e.key === "c" || e.key === "C") && (tab() === "board" || tab() === "inbox")) {
        e.preventDefault();
        setSheet({ kind: "new-card" });
      } else if (/^Digit[1-8]$/.test(e.code) && e.altKey) {
        // e.code: on macOS Option+1 types ¡, so e.key is no use here.
        e.preventDefault();
        setTab((["inbox", "board", "lanes", "terminal", "code", "ledger", "reports", "performance"] as const)[Number(e.code.slice(5)) - 1]!);
      }
    }
    if (e.key === "Escape") setMenu(false);
  };
  return onKey;
}
