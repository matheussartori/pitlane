// Free terminals: the model (xterms, layout, keeping it) lives in terminals/model.ts, dragging in
// terminals/drag.ts, and the screen in terminals/screen.tsx with its tabs and panes beside it.
// This file keeps the one name the shell imports from; what other screens need of the model
// (its settings, the servers it found) they take from terminals/model.ts.
export { terminalsWanting, startTerminals, openTerminal, terminalAt } from "./terminals/model";
export { TerminalScreen } from "./terminals/screen";
