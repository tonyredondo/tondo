// Live builds as they run, debugged with Debug Live or run with Run Live:
// when their Tondo files change, the program takes in the new code (Edit
// and Continue), and a status bar item says what became of each change:
// applied, applied with frames that keep the old code, needing the
// program started again, or not applied because the files do not compile.

import { ChildProcess, spawn } from 'node:child_process';
import * as os from 'node:os';
import * as vscode from 'vscode';
import { exec } from './cli';
import { tondo } from './extension';
import * as path from 'node:path';
import { HeldPanic, Reload, Shown, heldPlace, lineValues, liveHover, reloadShown, reloaded } from './report';

/** A live build: a debug session of `tondo dap`, or the program of a Run
 *  Live task, `tondo run --live`, which becomes it. */
type Live = { session: vscode.DebugSession } | { execution: vscode.TaskExecution; pid: number };

let current: Live | undefined;
let item: vscode.StatusBarItem;
let output: vscode.LogOutputChannel;
/** What the last reload did, or that it failed, until it is old news. */
let last: (Omit<Shown, 'state'> & { state: Shown['state'] | 'failed' | 'started' }) | undefined;
let fading: NodeJS.Timeout | undefined;
let waiting: NodeJS.Timeout | undefined;
let busy = false;
/** A change came while a reload ran, or with reloads on save off. */
let pending = false;

/** What the lines of the file in view left, beside them, while a live build
 *  runs and nothing stops. */
const beside = vscode.window.createTextEditorDecorationType({
  after: { margin: '0 0 0 2em', color: new vscode.ThemeColor('editorCodeLens.foreground') },
});
let showing = true;

type LineBindings = Record<string, { name: string; value: string }[]>;
type Looked = { values: LineBindings; held?: HeldPanic[] };

async function linesLeft(live: Live, file: string): Promise<Looked> {
  if ('session' in live) {
    return (await live.session.customRequest('values', { file })) as Looked;
  }
  const ran = await exec(['values', String(live.pid), file, '--for', '0', '--json'], os.tmpdir());
  return JSON.parse(ran.stdout) as Looked;
}

/** The lines a frame has not left yet: it goes on with the old code there. */
const oldCode = vscode.window.createTextEditorDecorationType({
  isWholeLine: true,
  overviewRulerColor: new vscode.ThemeColor('editorWarning.foreground'),
  overviewRulerLane: vscode.OverviewRulerLane.Left,
  backgroundColor: new vscode.ThemeColor('diffEditor.removedLineBackground'),
});
let stayLines: NonNullable<Reload['stay_lines']> = [];

function showStays(): void {
  for (const editor of vscode.window.visibleTextEditors) {
    const here = stayLines.filter(({ file }) => editor.document.uri.fsPath.endsWith(file));
    const options = here.flatMap(({ lines, why }) =>
      lines
        .filter((line) => line <= editor.document.lineCount)
        .map((line) => ({
          range: new vscode.Range(line - 1, 0, line - 1, 0),
          hoverMessage: `A frame stays on the old code here: ${why}`,
        })),
    );
    editor.setDecorations(oldCode, options);
  }
}

/** Expressions that say what they give as the program runs, nothing stopped,
 *  each by a `tondo eval` that goes on. */
type Watched = { expression: string; value: string; process?: ChildProcess };
const watched: Watched[] = [];
const watchesChanged = new vscode.EventEmitter<void>();
const liveWatches: vscode.TreeDataProvider<Watched> = {
  onDidChangeTreeData: watchesChanged.event,
  getChildren: () => watched,
  getTreeItem: ({ expression, value }) => {
    const item = new vscode.TreeItem(expression);
    item.description = value;
    item.contextValue = 'tondoLiveWatch';
    return item;
  },
};

async function pidOf(live: Live): Promise<number> {
  return 'session' in live ? ((await live.session.customRequest('pid')) as { pid: number }).pid : live.pid;
}

async function watchOne(one: Watched): Promise<void> {
  one.process?.kill();
  one.process = undefined;
  if (!current) {
    return;
  }
  const child = spawn(tondo(), ['eval', String(await pidOf(current)), one.expression, '--every', '1']);
  one.process = child;
  const show = (data: Buffer) => {
    const lines = data.toString().trim().split('\n');
    one.value = lines[lines.length - 1];
    watchesChanged.fire();
  };
  child.stdout.on('data', show);
  child.stderr.on('data', show);
}

async function addWatch(): Promise<void> {
  const expression = await vscode.window.showInputBox({ prompt: 'An expression to follow, as total(order())' });
  if (expression) {
    const one: Watched = { expression, value: '…' };
    watched.push(one);
    watchesChanged.fire();
    await watchOne(one);
  }
}

function removeWatch(one: Watched): void {
  one.process?.kill();
  watched.splice(watched.indexOf(one), 1);
  watchesChanged.fire();
}

/** What the lines of the file in view left, the last time it was asked. */
let lastLines: { file: string; lines: LineBindings } | undefined;
/** The panics that wait for a fix, in Problems and in the Live item. */
const panics = vscode.languages.createDiagnosticCollection('tondo-live');
let held: HeldPanic[] = [];

function showHeld(found: HeldPanic[], root: string): void {
  held = found;
  panics.clear();
  for (const panic of found) {
    const place = heldPlace(panic);
    if (place) {
      const uri = vscode.Uri.file(path.resolve(root, place.file));
      const line = new vscode.Range(place.line - 1, 0, place.line - 1, 1000);
      const problem = new vscode.Diagnostic(line, `${panic.message}: save a fix, and it starts again`);
      panics.set(uri, [...(panics.get(uri) ?? []), problem]);
    }
  }
  show();
}

/** Opens the line a panic waits at. */
async function openHeld(): Promise<void> {
  const place = held[0] && heldPlace(held[0]);
  const root = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath ?? '.';
  if (place) {
    const line = place.line - 1;
    const uri = vscode.Uri.file(path.resolve(root, place.file));
    await vscode.window.showTextDocument(uri, { selection: new vscode.Range(line, 0, line, 0) });
  }
}

async function showValues(): Promise<void> {
  const editor = vscode.window.activeTextEditor;
  const on = showing && vscode.workspace.getConfiguration('tondo').get<boolean>('liveValues', true);
  if (!current || !on || editor?.document.languageId !== 'tondo') {
    editor?.setDecorations(beside, []);
    return;
  }
  const looked = await linesLeft(current, editor.document.uri.fsPath).catch(() => ({ values: {} }) as Looked);
  const lines = looked.values;
  lastLines = { file: editor.document.uri.fsPath, lines };
  showHeld(looked.held ?? [], vscode.workspace.getWorkspaceFolder(editor.document.uri)?.uri.fsPath ?? '.');
  const options = Object.entries(lines).flatMap(([line, bindings]) => {
    const at = Number(line) - 1;
    const text = at < editor.document.lineCount ? lineValues(bindings, editor.document.lineAt(at).text) : '';
    const range = at < editor.document.lineCount ? editor.document.lineAt(at).range : undefined;
    return text && range
      ? [{ range: new vscode.Range(range.end, range.end), renderOptions: { after: { contentText: text } } }]
      : [];
  });
  editor.setDecorations(beside, options);
}

export function registerLive(context: vscode.ExtensionContext, channel: vscode.LogOutputChannel): void {
  output = channel;
  const looking = setInterval(() => void showValues(), 1000);
  context.subscriptions.push(
    beside,
    { dispose: () => clearInterval(looking) },
    panics,
    oldCode,
    vscode.window.registerTreeDataProvider('tondoLiveWatches', liveWatches),
    vscode.commands.registerCommand('tondo.addLiveWatch', addWatch),
    vscode.commands.registerCommand('tondo.removeLiveWatch', removeWatch),
    vscode.window.onDidChangeVisibleTextEditors(showStays),
    vscode.commands.registerCommand('tondo.openHeldPanic', openHeld),
    vscode.languages.registerHoverProvider('tondo', {
      provideHover: (document, position) => {
        const word = document.getWordRangeAtPosition(position);
        if (!word || !current || lastLines?.file !== document.uri.fsPath) {
          return undefined;
        }
        const shown = liveHover(lastLines.lines, document.getText(word), position.line + 1);
        return shown ? new vscode.Hover(`Live: \`${shown}\``) : undefined;
      },
    }),
    vscode.commands.registerCommand('tondo.liveValues', () => {
      showing = !showing;
      void showValues();
    }),
  );
  item = vscode.window.createStatusBarItem('tondo.live', vscode.StatusBarAlignment.Left, -1);
  item.name = 'Tondo Live';
  // Files an agent or another tool changes, too.
  const watcher = vscode.workspace.createFileSystemWatcher('**/*.to');
  context.subscriptions.push(
    item,
    watcher,
    vscode.commands.registerCommand('tondo.reload', () => reloadNow(true)),
    vscode.commands.registerCommand('tondo.restartLive', restart),
    vscode.debug.onDidStartDebugSession((session) => {
      if (session.type === 'tondo' && session.configuration.live) {
        begin({ session });
      }
    }),
    vscode.debug.onDidTerminateDebugSession((session) => {
      if (current && 'session' in current && current.session === session) {
        end();
      }
    }),
    vscode.tasks.onDidStartTaskProcess((event) => {
      const definition = event.execution.task.definition;
      if (definition.type === 'tondo' && definition.command === 'watch') {
        begin({ execution: event.execution, pid: event.processId });
      }
    }),
    vscode.tasks.onDidEndTaskProcess((event) => {
      if (current && 'execution' in current && current.execution === event.execution) {
        end();
      }
    }),
    vscode.workspace.onDidSaveTextDocument((document) => document.languageId === 'tondo' && changed()),
    watcher.onDidChange(changed),
    watcher.onDidCreate(changed),
    watcher.onDidDelete(changed),
    vscode.workspace.onDidChangeTextDocument((event) => event.document.languageId === 'tondo' && show()),
  );
}

function begin(live: Live): void {
  current = live;
  watched.forEach((one) => void watchOne(one));
  pending = false;
  void vscode.commands.executeCommand('setContext', 'tondo.live', true);
  show();
}

function end(): void {
  current = undefined;
  last = undefined;
  watched.forEach((one) => one.process?.kill());
  stayLines = [];
  showStays();
  showHeld([], '.');
  void vscode.commands.executeCommand('setContext', 'tondo.live', false);
  item.hide();
}

/** A file changed: the program takes it in once they stop changing. */
function changed(): void {
  if (!current) {
    return;
  }
  if (!vscode.workspace.getConfiguration('tondo').get<boolean>('reloadOnSave', true)) {
    pending = true;
    show();
    return;
  }
  clearTimeout(waiting);
  waiting = setTimeout(() => void reloadNow(false), 200);
}

/** Puts in the code of the files as they are now, and says what became of
 *  it; `asked`, by the command, says so when nothing changed. */
async function reloadNow(asked: boolean): Promise<void> {
  const live = current;
  if (!live) {
    if (asked) {
      void vscode.window.showInformationMessage(
        'Reload changes a live build while it runs: start one with Tondo: Debug Live or Tondo: Run Live.',
      );
    }
    return;
  }
  if (busy) {
    pending = true;
    return;
  }
  busy = true;
  pending = false;
  show();
  let shown: NonNullable<typeof last>;
  try {
    const reloaded = await reload(live);
    shown = reloadShown(reloaded);
    stayLines = reloaded.stay_lines ?? [];
    showStays();
    // What the change reaches, in the Testing view and in Problems.
    if (shown.state !== 'same' && vscode.workspace.getConfiguration('tondo').get<boolean>('testsOnSave', false)) {
      void vscode.commands.executeCommand('tondo.testsReached');
    }
  } catch (error) {
    shown = { state: 'failed', text: 'the reload failed', details: [(error as Error).message] };
  }
  busy = false;
  if (live !== current) {
    return;
  }
  output.info(`live: ${shown.text}${shown.details.map((line) => `\n  ${line}`).join('')}`);
  if (shown.state === 'same' && !asked) {
    last = undefined;
  } else {
    say(shown);
  }
  if (shown.state === 'restart') {
    // What came meanwhile goes in with the program started again.
    pending = false;
    await needsRestart(live, shown.details[0]);
  } else if (pending) {
    await reloadNow(false);
  }
}

async function reload(live: Live): Promise<Reload> {
  if ('session' in live) {
    return (await live.session.customRequest('reload')) as Reload;
  }
  const ran = await exec(['reload', String(live.pid), '--json'], os.tmpdir());
  try {
    return reloaded(ran.stdout);
  } catch (error) {
    throw new Error(ran.stdout ? (error as Error).message : ran.stderr.trim() || 'tondo reload failed');
  }
}

/** A change the program cannot take in while it runs: Run Live starts it
 *  again, as `tondo run --watch` does; Debug Live asks first. */
async function needsRestart(live: Live, why: string): Promise<void> {
  if ('execution' in live) {
    void vscode.window.showInformationMessage(`Tondo: the change could not go in while the program ran, so it started again: ${why}`);
    await restart();
    say({ state: 'started', text: 'started again', details: [why] });
    return;
  }
  void vscode.window
    .showWarningMessage(`Tondo: the change cannot go in while the program runs: ${why}`, 'Restart')
    .then((chosen) => (chosen && current === live ? restart() : undefined));
}

async function restart(): Promise<void> {
  const live = current;
  if (!live) {
    return;
  }
  if ('session' in live) {
    await vscode.debug.stopDebugging(live.session);
    await vscode.debug.startDebugging(live.session.workspaceFolder, live.session.configuration);
    return;
  }
  const ended = new Promise<void>((resolve) => {
    const listener = vscode.tasks.onDidEndTaskProcess((event) => {
      if (event.execution === live.execution) {
        listener.dispose();
        resolve();
      }
    });
  });
  live.execution.terminate();
  await ended;
  await vscode.tasks.executeTask(live.execution.task);
}

/** Shows what a reload did: for a while when all went in, until the next
 *  when something did not. */
function say(shown: NonNullable<typeof last>): void {
  last = shown;
  clearTimeout(fading);
  if (shown.state === 'applied' || shown.state === 'same' || shown.state === 'started') {
    fading = setTimeout(() => {
      last = undefined;
      show();
    }, 5000);
  }
  show();
}

const LOOKS: Record<NonNullable<typeof last>['state'], { icon: string; color?: string; command: string }> = {
  applied: { icon: 'check', command: 'tondo.showOutput' },
  same: { icon: 'check', command: 'tondo.showOutput' },
  started: { icon: 'debug-restart', color: 'statusBarItem.warningBackground', command: 'tondo.showOutput' },
  partly: { icon: 'warning', color: 'statusBarItem.warningBackground', command: 'tondo.showOutput' },
  restart: { icon: 'debug-restart', color: 'statusBarItem.warningBackground', command: 'tondo.restartLive' },
  errors: { icon: 'error', color: 'statusBarItem.errorBackground', command: 'workbench.actions.view.problems' },
  failed: { icon: 'error', color: 'statusBarItem.errorBackground', command: 'tondo.showOutput' },
};

function show(): void {
  if (!current) {
    return;
  }
  const debugged = 'session' in current;
  const onSave = vscode.workspace.getConfiguration('tondo').get<boolean>('reloadOnSave', true);
  const tooltip = new vscode.MarkdownString(
    `**Tondo Live** (${debugged ? 'Debug Live' : 'Run Live'}): ${
      onSave
        ? 'when a Tondo file is saved, the program takes in its new code and keeps its state.'
        : 'Tondo: Reload puts in the code of the files as they are.'
    }`,
  );
  tooltip.isTrusted = true;
  let text: string;
  item.backgroundColor = undefined;
  item.command = 'tondo.showOutput';
  if (held.length > 0) {
    item.text = `$(error) Live: a panic waits for a fix`;
    item.tooltip = held[0].message;
    item.command = 'tondo.openHeldPanic';
    item.backgroundColor = new vscode.ThemeColor('statusBarItem.errorBackground');
    item.show();
    return;
  }
  if (busy) {
    text = '$(sync~spin) Live: applying…';
  } else if (last) {
    const looks = LOOKS[last.state];
    text = `$(${looks.icon}) Live: ${last.text}`;
    item.backgroundColor = looks.color ? new vscode.ThemeColor(looks.color) : undefined;
    item.command = looks.command;
    tooltip.appendMarkdown(`\n\n${last.details.map((line) => `- ${line}`).join('\n')}`);
  } else {
    const dirty = vscode.workspace.textDocuments.some((d) => d.languageId === 'tondo' && d.isDirty);
    text = `$(zap) Live${dirty && onSave ? ': save to apply' : pending ? ': reload to apply' : ''}`;
    if (pending) {
      item.command = 'tondo.reload';
    }
  }
  tooltip.appendMarkdown(
    '\n\n[Reload](command:tondo.reload) · [Restart](command:tondo.restartLive) · [Show the Output](command:tondo.showOutput)',
  );
  item.text = text;
  item.tooltip = tooltip;
  item.show();
}
