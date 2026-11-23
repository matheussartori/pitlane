// Free terminals: the model (xterms, layout, keeping it) lives in terminals/model.ts, dragging in
// terminals/drag.ts, and the screen in terminals/screen.tsx with its tabs and panes beside it.
// This file keeps the one name the rest of the app imports from.
export {
  panesOf,
  without,
  insert,
  TERMINAL_FACES,
  terminalTabs,
  terminalsWanting,
  setTerminalNotify,
  terminalPrefs,
  setTerminalFace,
  setTerminalRender,
  setTerminalSide,
  setTerminalFont,
  retheme,
  startTerminals,
  openTerminal,
  terminalAt,
  localServers,
  resumeInTerminal,
  closePane,
  closeTab,
} from "./terminals/model";
export { TerminalScreen } from "./terminals/screen";
