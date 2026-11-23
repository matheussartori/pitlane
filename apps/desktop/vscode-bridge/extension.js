// Pitlane's bridge inside its VS Code: "Ask Claude about this" (⌘L, or the editor's menu) asks what
// should change, then leaves the ask, the file, the lines and the selected text as a JSON file in
// the folder Pitlane named (PITLANE_BRIDGE, set when it started code-server). Pitlane picks it up
// and makes the card. Nothing leaves the machine; nothing runs without you.
const vscode = require("vscode");
const fs = require("fs");
const path = require("path");

const MAX_TEXT = 12000;

function activate(ctx) {
  ctx.subscriptions.push(
    vscode.commands.registerCommand("pitlane.ask", async () => {
      const ed = vscode.window.activeTextEditor;
      if (!ed) return;
      const dir = process.env.PITLANE_BRIDGE;
      if (!dir) {
        void vscode.window.showWarningMessage("Pitlane did not start this VS Code, so it cannot take the card.");
        return;
      }
      const doc = ed.document;
      const sel = ed.selection;
      // No selection: the line the cursor is on.
      const range = sel.isEmpty ? doc.lineAt(sel.active.line).range : sel;
      // A selection that ends at the start of a line ends on the line before.
      const last = !sel.isEmpty && sel.end.character === 0 && sel.end.line > sel.start.line ? sel.end.line - 1 : range.end.line;
      const from = range.start.line + 1;
      const to = last + 1;
      const name = path.basename(doc.uri.fsPath);
      const ask = await vscode.window.showInputBox({
        title: from === to ? `Ask Claude about ${name}:${from}` : `Ask Claude about ${name}:${from}-${to}`,
        prompt: "What should change here? Pitlane makes it a card on the Board.",
        placeHolder: "Handle an empty list here",
        ignoreFocusOut: true,
      });
      if (!ask || !ask.trim()) return;
      let text = doc.getText(range);
      if (text.length > MAX_TEXT) text = `${text.slice(0, MAX_TEXT)}\n…`;
      const folder = vscode.workspace.getWorkspaceFolder(doc.uri);
      const req = {
        ask: ask.trim(),
        file: doc.uri.fsPath,
        root: folder ? folder.uri.fsPath : "",
        from,
        to,
        language: doc.languageId,
        text,
      };
      try {
        fs.mkdirSync(dir, { recursive: true });
        const base = path.join(dir, `${Date.now()}-${process.pid}.json`);
        // Written whole, then renamed: Pitlane never reads half a request.
        fs.writeFileSync(`${base}.part`, JSON.stringify(req));
        fs.renameSync(`${base}.part`, base);
        vscode.window.setStatusBarMessage("$(check) Sent to Pitlane", 3000);
      } catch (e) {
        void vscode.window.showErrorMessage(`Pitlane did not get it: ${e && e.message ? e.message : e}`);
      }
    }),
  );
}

function deactivate() {}

module.exports = { activate, deactivate };
