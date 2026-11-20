// Browser preview (vite dev without Tauri): answers the commands from a fixture made by
// `pit lanes --json` on a throwaway demo repo, so the UI can be designed and linted in a browser.
// Never part of a production build: api.ts only imports it under import.meta.env.DEV.
import yard from "./yard.json";

const repo = { path: "/Users/you/Projects/demo", name: "demo" };

export async function preview<T>(cmd: string): Promise<T> {
  await new Promise((r) => setTimeout(r, 250));
  switch (cmd) {
    case "settings":
      return { repos: [repo], last: repo.path } as T;
    case "select_repo":
      return undefined as T;
    case "yard": {
      // ?state=loading|empty|error previews the other three states of the screen.
      const state = new URLSearchParams(location.search).get("state");
      if (state === "loading") return new Promise<T>(() => {});
      if (state === "error") throw "Could not fetch origin: ssh: connect to host github.com port 22: Network is unreachable";
      if (state === "empty") return { ...yard, fetched: true, lanes: [], only_in_integration: [] } as T;
      return { ...yard, fetched: true } as T;
    }
    default:
      throw `Preview mode: "${cmd}" runs only in the desktop app.`;
  }
}
