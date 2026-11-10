// What a release build makes of the code, in the editor: `tondo
// optimizations` of the program of the file open (toolchain 7.18). The end
// of each line says what costs there, what allocates, a check a loop keeps
// or a call that stays, and over the line is all the passes said of it.
// Saving a file of the program tells it again.

import * as path from 'node:path';
import * as vscode from 'vscode';
import { exec, programFor } from './cli';
import { OptimizedLine, optimizedLines } from './report';

export function registerOptimizations(context: vscode.ExtensionContext, output: vscode.LogOutputChannel): void {
  const decoration = vscode.window.createTextEditorDecorationType({
    after: { margin: '0 0 0 2em', color: new vscode.ThemeColor('editorCodeLens.foreground') },
  });
  // The lines told, by file, and the file whose program they are of.
  let shown = new Map<string, OptimizedLine[]>();
  let told: string | undefined;
  const decorate = (editors: readonly vscode.TextEditor[]) => {
    for (const editor of editors) {
      const lines = shown.get(editor.document.uri.fsPath) ?? [];
      editor.setDecorations(
        decoration,
        lines
          .filter(({ line }) => line <= editor.document.lineCount)
          .map(({ line, end, said }) => {
            const range = editor.document.lineAt(line - 1).range;
            const hover = new vscode.MarkdownString(said.map((text) => `- ${text}`).join('\n'));
            return {
              range,
              hoverMessage: hover,
              renderOptions: { after: { contentText: end ? `⚙ ${end}` : '' } },
            };
          }),
      );
    }
  };
  // The lines told, or none when tondo could not tell them.
  const tell = async (file: string): Promise<number | undefined> => {
    told = file;
    const { program, cwd } = await programFor(file);
    const ran = await exec(['optimizations', program, '--json'], cwd);
    const dir = path.extname(program) === '.to' ? path.dirname(program) : program;
    let lines;
    try {
      lines = optimizedLines(ran.stdout, dir);
    } catch {
      void vscode.window.showErrorMessage(ran.stderr.trim() || `tondo optimizations ${program} failed`);
      return undefined;
    }
    shown = new Map();
    for (const line of lines) {
      shown.set(line.file, [...(shown.get(line.file) ?? []), line]);
    }
    decorate(vscode.window.visibleTextEditors);
    void vscode.commands.executeCommand('setContext', 'tondo.optimized', true);
    output.info(`optimizations of ${program}: ${lines.length} lines`);
    return lines.length;
  };

  context.subscriptions.push(
    decoration,
    vscode.window.onDidChangeVisibleTextEditors(decorate),
    vscode.workspace.onDidSaveTextDocument((document) => {
      if (told && document.languageId === 'tondo') {
        void tell(told);
      }
    }),
    vscode.commands.registerCommand('tondo.showOptimizations', async (uri?: vscode.Uri) => {
      const file = uri?.scheme === 'file' ? uri.fsPath : vscode.window.activeTextEditor?.document.uri.fsPath;
      if (!file?.endsWith('.to')) {
        void vscode.window.showInformationMessage('Open a Tondo file to see what a release build makes of its program.');
        return undefined;
      }
      await vscode.workspace.saveAll(false);
      return tell(file);
    }),
    vscode.commands.registerCommand('tondo.clearOptimizations', () => {
      shown = new Map();
      told = undefined;
      decorate(vscode.window.visibleTextEditors);
      void vscode.commands.executeCommand('setContext', 'tondo.optimized', false);
    }),
  );
}
