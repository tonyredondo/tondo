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
import { Failure, counterRows, dumpPage, failureMarks, logpointArgs, running, traced } from './report';

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
        (await vscode.window.showOpenDialog({ filters: { 'Tondo dumps': ['json', 'gz'], 'Cores and all files': ['*'] }, openLabel: 'Debug this dump' }))?.[0];
      if (file) {
        const folder = vscode.workspace.getWorkspaceFolder(file) ?? vscode.workspace.workspaceFolders?.[0];
        const cwd = folder?.uri.fsPath ?? path.dirname(file.fsPath);
        await vscode.debug.startDebugging(folder, { type: 'tondo', request: 'launch', name: 'Debug a dump', dump: file.fsPath, cwd });
      }
    }),
    vscode.commands.registerCommand('tondo.dumpReport', dumpReport),
    vscode.commands.registerCommand('tondo.testFromDump', testFromDump),
    vscode.commands.registerCommand('tondo.failureIssue', failureIssue),
    vscode.commands.registerCommand('tondo.redactDump', redactDump),
    vscode.commands.registerCommand('tondo.readCore', readCore),
    vscode.commands.registerCommand('tondo.blocked', blocked),
    vscode.commands.registerCommand('tondo.serveDumps', serveDumps),
    { dispose: () => serving?.kill() },
    vscode.commands.registerCommand('tondo.logpoint', logpoint),
    vscode.commands.registerCommand('tondo.showFailures', showFailures),
    vscode.commands.registerCommand('tondo.clearFailures', () => failures.clear()),
    failures,
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

/** The lines where the dumps of a directory fell. */
const failures = vscode.languages.createDiagnosticCollection('tondo-failures');

/** Marks the lines the dumps of a directory fell at, in the code they came
 *  from, with how many times and since when. */
async function showFailures(): Promise<void> {
  const picked = await vscode.window.showOpenDialog({
    canSelectFolders: true,
    canSelectFiles: false,
    openLabel: 'The directory of the dumps',
  });
  const root = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  const dir = picked?.[0].fsPath;
  if (!dir || !root) {
    return;
  }
  const prompt = 'Only those new after this commit; empty, all of them';
  const since = await vscode.window.showInputBox({ prompt });
  if (since === undefined) {
    return;
  }
  const ran = await exec(['dump', 'group', dir, ...(since ? ['--since', since] : []), '--json'], root);
  if (ran.code !== 0) {
    void vscode.window.showErrorMessage(ran.stderr.trim() || 'tondo dump group failed');
    return;
  }
  const marks = failureMarks((JSON.parse(ran.stdout) as { groups: Failure[] }).groups);
  const byFile = new Map<string, vscode.Diagnostic[]>();
  for (const { file, line, text } of marks) {
    const range = new vscode.Range(line - 1, 0, line - 1, Number.MAX_SAFE_INTEGER);
    const mark = new vscode.Diagnostic(range, text, vscode.DiagnosticSeverity.Warning);
    mark.source = 'tondo dumps';
    byFile.set(file, [...(byFile.get(file) ?? []), mark]);
  }
  failures.clear();
  for (const [file, marked] of byFile) {
    failures.set(vscode.Uri.file(path.resolve(root, file)), marked);
  }
  void vscode.window.showInformationMessage(`${marks.length} failures marked in the code.`);
}

const logpoints = vscode.window.createOutputChannel('Tondo Logpoints');

/** What the line of the cursor holds in a running program, each time it goes
 *  by, in a panel, until it reaches its limit or the person stops it. */
async function logpoint(): Promise<void> {
  const editor = vscode.window.activeTextEditor;
  if (editor?.document.languageId !== 'tondo') {
    void vscode.window.showInformationMessage('Put the cursor on the line of a Tondo file.');
    return;
  }
  const pid = await pickProcess();
  const prompt = 'What to say each time, as `order {order.id}`; empty, every binding';
  const text = pid && (await vscode.window.showInputBox({ prompt }));
  if (!pid || text === undefined) {
    return;
  }
  const condition = await vscode.window.showInputBox({ prompt: 'Only when this holds, as `order.total > 100`; empty, always' });
  if (condition === undefined) {
    return;
  }
  const whole = await vscode.window.showInputBox({
    prompt: 'A file for a dump of the whole program the first time; empty, none',
  });
  if (whole === undefined) {
    return;
  }
  const file = vscode.workspace.asRelativePath(editor.document.uri, false);
  const place = `${file}:${editor.selection.active.line + 1}`;
  logpoints.show(true);
  logpoints.appendLine(`a logpoint on ${place}`);
  const title = `A logpoint on ${place}: Cancel takes it out`;
  const options = { location: vscode.ProgressLocation.Notification, title, cancellable: true };
  const ran = await vscode.window.withProgress(options, (_, token) =>
    exec(logpointArgs(pid, place, text, condition, whole ? 1 : 100, whole), path.dirname(editor.document.uri.fsPath), token, (line) =>
      logpoints.appendLine(line),
    ),
  );
  if (ran.code !== 0 && ran.code !== null) {
    void vscode.window.showErrorMessage(ran.stderr.trim() || 'tondo logpoint failed');
  }
}

/** The dump to work on, picked, and the project it is of. */
async function pickedDump(): Promise<{ file: string; root: string } | undefined> {
  const picked = await vscode.window.showOpenDialog({
    filters: { 'Tondo dumps': ['json', 'gz'], 'Cores and all files': ['*'] },
    openLabel: 'The dump',
  });
  const file = picked?.[0].fsPath;
  return file ? { file, root: vscode.workspace.workspaceFolders?.[0]?.uri.fsPath ?? path.dirname(file) } : undefined;
}

/** A test that fails as a dump did, in the module's test file. */
async function testFromDump(): Promise<void> {
  const dump = await pickedDump();
  if (!dump) {
    return;
  }
  const ran = await exec(['dump', 'test', dump.file, '--json'], dump.root);
  if (ran.code !== 0) {
    void vscode.window.showErrorMessage(ran.stderr.trim() || 'tondo dump test failed');
    return;
  }
  const made = JSON.parse(ran.stdout) as { file: string; test: string; reproduces: boolean };
  const opened = await vscode.workspace.openTextDocument(path.resolve(dump.root, made.file));
  await vscode.window.showTextDocument(opened, vscode.ViewColumn.Beside);
  const said = made.reproduces ? 'fails as the dump did' : 'passes: the failure takes more than these values';
  void vscode.window.showInformationMessage(`\`${made.test}\` ${said}.`);
}

/** A failure of a directory of dumps as an issue, beside the code. */
async function failureIssue(): Promise<void> {
  const picked = await vscode.window.showOpenDialog({
    canSelectFolders: true,
    canSelectFiles: false,
    openLabel: 'The directory of the dumps',
  });
  const dir = picked?.[0].fsPath;
  if (!dir) {
    return;
  }
  const root = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath ?? dir;
  const listed = await exec(['dump', 'group', dir, '--json'], root);
  if (listed.code !== 0) {
    void vscode.window.showErrorMessage(listed.stderr.trim() || 'tondo dump group failed');
    return;
  }
  const groups = (JSON.parse(listed.stdout) as { groups: (Failure & { signature: string })[] }).groups;
  const items = groups.map((group) => ({ label: group.message ?? group.signature, detail: group.at ?? '', group }));
  const chosen = await vscode.window.showQuickPick(items, { placeHolder: 'The failure' });
  if (!chosen) {
    return;
  }
  const ran = await exec(['dump', 'issue', dir, chosen.group.signature], root);
  if (ran.code !== 0) {
    void vscode.window.showErrorMessage(ran.stderr.trim() || 'tondo dump issue failed');
    return;
  }
  const document = await vscode.workspace.openTextDocument({ content: ran.stdout, language: 'markdown' });
  await vscode.window.showTextDocument(document, vscode.ViewColumn.Beside);
}

/** A dump to share, without what the program held. */
async function redactDump(): Promise<void> {
  const dump = await pickedDump();
  const output = dump && (await vscode.window.showSaveDialog({ saveLabel: 'The dump to share' }));
  if (!dump || !output) {
    return;
  }
  const ran = await exec(['dump', 'redact', dump.file, output.fsPath], dump.root);
  if (ran.code !== 0) {
    void vscode.window.showErrorMessage(ran.stderr.trim() || 'tondo dump redact failed');
  }
}

/** A core of the system as a dump, with the program it is of, opened to
 *  debug. */
async function readCore(): Promise<void> {
  const core = await vscode.window.showOpenDialog({ openLabel: 'The core' });
  const program = core && (await vscode.window.showOpenDialog({ openLabel: 'Its program' }));
  if (!core || !program) {
    return;
  }
  const output = `${core[0].fsPath}.dump.json`;
  const args = ['dump', 'core', core[0].fsPath, '--executable', program[0].fsPath, '--output', output];
  const ran = await exec(args, path.dirname(core[0].fsPath));
  if (ran.code !== 0) {
    void vscode.window.showErrorMessage(ran.stderr.trim() || 'tondo dump core failed');
    return;
  }
  await vscode.commands.executeCommand('tondo.debugDump', vscode.Uri.file(output));
}

/** The page of failures this window serves, one at a time. */
let serving: ReturnType<typeof spawn> | undefined;

/** The failures of a directory as a page that stays up to date. */
async function serveDumps(): Promise<void> {
  const picked = await vscode.window.showOpenDialog({
    canSelectFolders: true,
    canSelectFiles: false,
    openLabel: 'The directory of the dumps',
  });
  const dir = picked?.[0].fsPath;
  if (!dir) {
    return;
  }
  serving?.kill();
  const child = spawn(tondo(), ['dump', 'serve', dir]);
  serving = child;
  child.stdout?.once('data', (data: Buffer) => {
    void vscode.env.openExternal(vscode.Uri.parse(data.toString().trim()));
  });
}

/** What the tasks of a running program wait for, beside the code. */
async function blocked(): Promise<void> {
  const pid = await pickProcess();
  if (!pid) {
    return;
  }
  const ran = await exec(['blocked', pid], os.tmpdir());
  if (ran.code !== 0) {
    void vscode.window.showErrorMessage(ran.stderr.trim() || 'tondo blocked failed');
    return;
  }
  await show(ran.stdout);
}

/** A dump as one page beside the code, whose places open: here, or from the
 *  repository they came from, as `tondo source` brings them. */
async function dumpReport(uri?: vscode.Uri): Promise<void> {
  const file =
    uri?.fsPath ??
    (await vscode.window.showOpenDialog({ filters: { 'Tondo dumps': ['json', 'gz'], 'Cores and all files': ['*'] }, openLabel: 'Read this dump' }))?.[0].fsPath;
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
    (await vscode.window.showOpenDialog({ filters: { 'Tondo dumps': ['json', 'gz'], 'Cores and all files': ['*'] }, openLabel }))?.[0].fsPath;
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
