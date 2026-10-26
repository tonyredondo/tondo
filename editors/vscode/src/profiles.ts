// Profiles in the editor: `tondo profile` of a program, `tondo test
// --profile` of tests and benchmarks, or any pprof file opened. The share
// of each hot line of the program shows at its end, and the flame graph of
// `tondo profile show --html` in a panel.

import * as fs from 'node:fs/promises';
import * as os from 'node:os';
import * as path from 'node:path';
import * as vscode from 'vscode';
import { exec, programFor, settings } from './cli';
import { tondo } from './extension';
import { hotLines, share } from './report';

/** What shows a profile: given the project's directory and the file. */
export type ShowProfile = (dir: string, file: string) => Promise<void>;

export function registerProfiles(context: vscode.ExtensionContext, output: vscode.LogOutputChannel): ShowProfile {
  const decoration = vscode.window.createTextEditorDecorationType({
    after: { margin: '0 0 0 2em', color: new vscode.ThemeColor('editorCodeLens.foreground') },
  });
  // The lines of the profile shown, by file.
  let shown = new Map<string, { line: number; text: string }[]>();
  let panel: vscode.WebviewPanel | undefined;
  const decorate = (editors: readonly vscode.TextEditor[]) => {
    for (const editor of editors) {
      const lines = shown.get(editor.document.uri.fsPath) ?? [];
      editor.setDecorations(
        decoration,
        lines
          .filter(({ line }) => line <= editor.document.lineCount)
          .map(({ line, text }) => {
            const end = editor.document.lineAt(line - 1).range.end;
            return { range: new vscode.Range(end, end), renderOptions: { after: { contentText: text } } };
          }),
      );
    }
  };

  const show: ShowProfile = async (dir, file) => {
    const html = path.join(os.tmpdir(), `tondo-flame-${process.pid}.html`);
    const ran = await exec(['profile', 'show', file, '--json', '--top', '500', '--html', html], dir);
    let hot;
    try {
      hot = hotLines(ran.stdout);
    } catch {
      void vscode.window.showErrorMessage(ran.stderr.trim() || `tondo cannot read the profile ${file}`);
      return;
    }
    shown = new Map();
    for (const line of hot.lines) {
      const text = share(line.flat, line.cum, hot.total, hot.unit);
      if (text) {
        const file = path.resolve(dir, line.file);
        shown.set(file, [...(shown.get(file) ?? []), { line: line.line, text }]);
      }
    }
    decorate(vscode.window.visibleTextEditors);
    void vscode.commands.executeCommand('setContext', 'tondo.profiled', true);
    output.info(`profile ${file}: ${hot.type}, ${hot.lines.length} lines of the program`);
    panel ??= vscode.window.createWebviewPanel('tondoProfile', 'Tondo Profile', vscode.ViewColumn.Beside, {});
    panel.onDidDispose(() => (panel = undefined));
    panel.title = `Profile: ${path.basename(file)}`;
    panel.webview.html = await fs.readFile(html, 'utf8').catch(() => '');
    panel.reveal(vscode.ViewColumn.Beside, true);
  };

  context.subscriptions.push(
    decoration,
    vscode.window.onDidChangeVisibleTextEditors(decorate),
    // A program profiled until it ends, in a terminal it can be stopped in.
    vscode.commands.registerCommand('tondo.profile', async (uri?: vscode.Uri) => {
      const file = uri?.scheme === 'file' ? uri.fsPath : vscode.window.activeTextEditor?.document.uri.fsPath;
      if (!file?.endsWith('.to')) {
        void vscode.window.showInformationMessage('Open a Tondo file to profile its program.');
        return;
      }
      await vscode.workspace.saveAll(false);
      const { program, cwd } = await programFor(file);
      const profile = path.join(os.tmpdir(), `tondo-profile-${process.pid}-${Date.now()}.pprof`);
      const run = settings('run', file);
      const execution = new vscode.ProcessExecution(tondo(), ['profile', '--output', profile, '--', program, ...run.args], { cwd, env: run.env });
      const folder = vscode.workspace.getWorkspaceFolder(vscode.Uri.file(file));
      const task = new vscode.Task({ type: 'tondo', command: 'profile' }, folder ?? vscode.TaskScope.Workspace, `profile ${path.basename(program)}`, 'tondo', execution);
      task.presentationOptions = { focus: true, panel: vscode.TaskPanelKind.Dedicated, clear: true };
      const started = await vscode.tasks.executeTask(task);
      const ended = vscode.tasks.onDidEndTaskProcess(async (event) => {
        if (event.execution === started) {
          ended.dispose();
          await show(cwd, profile);
        }
      });
      context.subscriptions.push(ended);
    }),
    vscode.commands.registerCommand('tondo.showProfile', async () => {
      const picked = await vscode.window.showOpenDialog({ filters: { Profiles: ['pprof', 'pb.gz', 'prof'] }, openLabel: 'Show the Profile' });
      if (picked?.[0]) {
        const folder = vscode.workspace.getWorkspaceFolder(picked[0])?.uri.fsPath ?? path.dirname(picked[0].fsPath);
        await show(folder, picked[0].fsPath);
      }
    }),
    vscode.commands.registerCommand('tondo.clearProfile', () => {
      shown = new Map();
      decorate(vscode.window.visibleTextEditors);
      panel?.dispose();
      void vscode.commands.executeCommand('setContext', 'tondo.profiled', false);
    }),
  );
  return show;
}
