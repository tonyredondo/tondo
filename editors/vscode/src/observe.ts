// Running programs, as `tondo ps`, `counters`, `trace` and `dump` observe
// them: a picker of programs to attach to, their counters in a view as
// they change, a trace opened in Perfetto, and a dump's summary.

import { spawn } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import * as http from 'node:http';
import * as os from 'node:os';
import * as path from 'node:path';
import * as vscode from 'vscode';
import { exec } from './cli';
import { tondo } from './extension';
import { counterRows, dumpPage, running, traced } from './report';

/** A running program, picked: its process ID, or nothing. */
export async function pickProcess(): Promise<string | undefined> {
  const ran = await exec(['ps', '--json'], os.tmpdir());
  const programs = running(ran.stdout);
  if (programs.length === 0) {
    void vscode.window.showInformationMessage(
      'No Tondo program is running that tools can see: start one with TONDO_DIAGNOSTICS=1, or a debug build.',
    );
    return undefined;
  }
  const items = programs.map((program) => ({
    label: path.basename(program.program),
    description: `process ${program.pid}`,
    detail: [program.program, ...program.arguments].join(' '),
    pid: program.pid,
  }));
  const picked = await vscode.window.showQuickPick(items, { placeHolder: 'A running Tondo program' });
  return picked?.pid.toString();
}

/** The counters of the program watched, as `tondo counters` gives them. */
class Counters implements vscode.TreeDataProvider<{ name: string; value: string }> {
  rows: { name: string; value: string }[] = [];
  readonly changed = new vscode.EventEmitter<void>();
  readonly onDidChangeTreeData = this.changed.event;

  getTreeItem(row: { name: string; value: string }): vscode.TreeItem {
    const item = new vscode.TreeItem(row.name);
    item.description = row.value;
    return item;
  }

  getChildren(): { name: string; value: string }[] {
    return this.rows;
  }
}

export function registerObserving(context: vscode.ExtensionContext, output: vscode.LogOutputChannel): void {
  const counters = new Counters();
  let watching: ReturnType<typeof spawn> | undefined;
  const stop = () => {
    watching?.kill();
    watching = undefined;
    void vscode.commands.executeCommand('setContext', 'tondo.counting', false);
  };
  context.subscriptions.push(
    vscode.window.registerTreeDataProvider('tondoCounters', counters),
    vscode.commands.registerCommand('tondo.pickProcess', pickProcess),
    vscode.commands.registerCommand('tondo.watchCounters', async () => {
      const pid = await pickProcess();
      if (!pid) {
        return;
      }
      stop();
      const child = spawn(tondo(), ['counters', pid, '--json']);
      watching = child;
      void vscode.commands.executeCommand('setContext', 'tondo.counting', true);
      let pending = '';
      child.stdout.on('data', (data: Buffer) => {
        const lines = (pending + data.toString()).split('\n');
        pending = lines.pop() ?? '';
        const rows = lines.map(counterRows).filter((rows) => rows !== undefined).pop();
        if (rows) {
          counters.rows = rows;
          counters.changed.fire();
        }
      });
      child.stderr.on('data', (data: Buffer) => output.appendLine(data.toString().trimEnd()));
      child.on('close', () => {
        if (watching === child) {
          stop();
        }
      });
    }),
    vscode.commands.registerCommand('tondo.stopCounters', stop),
    vscode.commands.registerCommand('tondo.trace', trace),
    vscode.commands.registerCommand('tondo.dump', dump),
    vscode.commands.registerCommand('tondo.debugDump', async (uri?: vscode.Uri) => {
      const file =
        uri ??
        (await vscode.window.showOpenDialog({ filters: { 'Tondo dumps': ['json', 'gz'] }, openLabel: 'Debug this dump' }))?.[0];
      if (file) {
        const folder = vscode.workspace.getWorkspaceFolder(file) ?? vscode.workspace.workspaceFolders?.[0];
        const cwd = folder?.uri.fsPath ?? path.dirname(file.fsPath);
        await vscode.debug.startDebugging(folder, { type: 'tondo', request: 'launch', name: 'Debug a dump', dump: file.fsPath, cwd });
      }
    }),
    vscode.commands.registerCommand('tondo.dumpReport', dumpReport),
    vscode.commands.registerCommand('tondo.compareDumps', compareDumps),
    vscode.commands.registerCommand('tondo.dumpTrend', dumpTrend),
    { dispose: stop },
  );
}

/** What a command says, in a document beside the code. */
async function show(content: string): Promise<void> {
  const document = await vscode.workspace.openTextDocument({ content, language: 'plaintext' });
  await vscode.window.showTextDocument(document, vscode.ViewColumn.Beside);
}

/** A dump as one page beside the code, whose places open: here, or from the
 *  repository they came from, as `tondo source` brings them. */
async function dumpReport(uri?: vscode.Uri): Promise<void> {
  const file =
    uri?.fsPath ??
    (await vscode.window.showOpenDialog({ filters: { 'Tondo dumps': ['json', 'gz'] }, openLabel: 'Read this dump' }))?.[0].fsPath;
  if (!file) {
    return;
  }
  const folder = vscode.workspace.getWorkspaceFolder(vscode.Uri.file(file)) ?? vscode.workspace.workspaceFolders?.[0];
  const cwd = folder?.uri.fsPath ?? path.dirname(file);
  const ran = await exec(['dump', 'report', file], cwd);
  if (ran.code !== 0) {
    void vscode.window.showErrorMessage(ran.stderr.trim() || 'tondo dump report failed');
    return;
  }
  const options = { enableScripts: true, retainContextWhenHidden: true };
  const panel = vscode.window.createWebviewPanel('tondoDump', path.basename(file), vscode.ViewColumn.Beside, options);
  panel.webview.html = dumpPage(ran.stdout, randomBytes(16).toString('hex'));
  // While it is open, a hover says what its frames held.
  void vscode.commands.executeCommand('tondo.useDump', file);
  panel.onDidDispose(() => void vscode.commands.executeCommand('tondo.useDump', null));
  panel.webview.onDidReceiveMessage(async (asked: { file: string; line: number }) => {
    const found = await exec(['source', asked.file, '--from', file, '--json'], cwd);
    if (found.code !== 0) {
      void vscode.window.showErrorMessage(found.stderr.trim() || `${asked.file} is not here`);
      return;
    }
    const document = await vscode.workspace.openTextDocument((JSON.parse(found.stdout) as { path: string }).path);
    const at = new vscode.Position(Math.max(asked.line - 1, 0), 0);
    await vscode.window.showTextDocument(document, { selection: new vscode.Range(at, at), viewColumn: vscode.ViewColumn.One });
  });
}

/** What changed from one dump of a program to a later one. */
async function compareDumps(): Promise<void> {
  const pick = async (openLabel: string) =>
    (await vscode.window.showOpenDialog({ filters: { 'Tondo dumps': ['json', 'gz'] }, openLabel }))?.[0].fsPath;
  const first = await pick('The earlier dump');
  const second = first && (await pick('The later dump'));
  if (!first || !second) {
    return;
  }
  const ran = await exec(['dump', 'diff', first, second], path.dirname(first));
  if (ran.code !== 0) {
    void vscode.window.showErrorMessage(ran.stderr.trim() || 'tondo dump diff failed');
    return;
  }
  await show(ran.stdout);
}

/** What keeps growing over the dumps of a program in a directory. */
async function dumpTrend(): Promise<void> {
  const picked = await vscode.window.showOpenDialog({
    canSelectFolders: true,
    canSelectFiles: false,
    openLabel: 'The directory of the dumps',
  });
  const dir = picked?.[0].fsPath;
  if (!dir) {
    return;
  }
  const ran = await exec(['dump', 'trend', dir], dir);
  if (ran.code !== 0) {
    void vscode.window.showErrorMessage(ran.stderr.trim() || 'tondo dump trend failed');
    return;
  }
  await show(ran.stdout);
}

/** Records a running program for a while, and opens the timeline in
 *  Perfetto. */
async function trace(): Promise<void> {
  const pid = await pickProcess();
  if (!pid) {
    return;
  }
  const seconds = await vscode.window.showInputBox({ prompt: 'Seconds to record', value: '5', validateInput: (v) => (Number(v) > 0 ? undefined : 'A number of seconds') });
  if (!seconds) {
    return;
  }
  const output = path.join(os.tmpdir(), `tondo-trace-${pid}-${Date.now()}.json`);
  const ran = await vscode.window.withProgress(
    { location: vscode.ProgressLocation.Notification, title: `Tracing process ${pid} for ${seconds} s` },
    () => exec(['trace', pid, '--duration', seconds, '--output', output, '--json'], os.tmpdir()),
  );
  let files;
  try {
    files = traced(ran.stdout);
  } catch (error) {
    void vscode.window.showErrorMessage(ran.stderr.trim() || (error as Error).message);
    return;
  }
  const choice = await vscode.window.showInformationMessage(
    `The trace is in ${files.timeline}, and its CPU profile in ${files.profile}.`,
    'Open in Perfetto',
    'Show the Files',
  );
  if (choice === 'Open in Perfetto') {
    await perfetto(files.timeline);
  } else if (choice === 'Show the Files') {
    await vscode.commands.executeCommand('revealFileInOS', vscode.Uri.file(files.timeline));
  }
}

/** Opens a trace in Perfetto's UI, which loads it from this machine's port
 *  9001, the one it allows, served once. */
async function perfetto(file: string): Promise<void> {
  const name = path.basename(file);
  const server = http.createServer((request, response) => {
    response.setHeader('Access-Control-Allow-Origin', 'https://ui.perfetto.dev');
    if (request.url !== `/${name}`) {
      response.writeHead(404).end();
      return;
    }
    void import('node:fs').then((fs) => {
      fs.createReadStream(file).pipe(response).on('finish', () => server.close());
    });
  });
  try {
    await new Promise<void>((resolve, reject) => server.once('error', reject).listen(9001, '127.0.0.1', resolve));
  } catch (error) {
    void vscode.window.showErrorMessage(`Cannot serve the trace on port 9001: ${(error as Error).message}. Open ${file} in https://ui.perfetto.dev.`);
    return;
  }
  setTimeout(() => server.close(), 60_000).unref();
  const url = `https://ui.perfetto.dev/#!/?url=http://127.0.0.1:9001/${encodeURIComponent(name)}`;
  await vscode.env.openExternal(vscode.Uri.parse(url));
}

/** Writes a dump of a running program and shows its summary. */
async function dump(): Promise<void> {
  const pid = await pickProcess();
  if (!pid) {
    return;
  }
  const file = path.join(os.tmpdir(), `tondo-${pid}-${Date.now()}.dump.json`);
  const ran = await exec(['dump', pid, '--output', file, '--json'], os.tmpdir());
  let summary: string;
  try {
    const dumped = JSON.parse(ran.stdout) as { format?: string; summary?: string };
    if (dumped.format !== 'tondo-dump-0.1/1') {
      throw new Error();
    }
    summary = dumped.summary ?? '';
  } catch {
    void vscode.window.showErrorMessage(ran.stderr.trim() || 'tondo dump gave no dump');
    return;
  }
  const document = await vscode.workspace.openTextDocument({ content: `${summary}\n\nThe dump: ${file}\n`, language: 'plaintext' });
  await vscode.window.showTextDocument(document);
}
