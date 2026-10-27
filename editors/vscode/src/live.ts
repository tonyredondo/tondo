// Live builds as they run, debugged with Debug Live or run with Run Live:
// when their Tondo files change, the program takes in the new code (Edit
// and Continue), and a status bar item says what became of each change:
// applied, applied with frames that keep the old code, needing the
// program started again, or not applied because the files do not compile.

import * as os from 'node:os';
import * as vscode from 'vscode';
import { exec } from './cli';
import { Reload, Shown, reloadShown, reloaded } from './report';

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

export function registerLive(context: vscode.ExtensionContext, channel: vscode.LogOutputChannel): void {
  output = channel;
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
  pending = false;
  void vscode.commands.executeCommand('setContext', 'tondo.live', true);
  show();
}

function end(): void {
  current = undefined;
  last = undefined;
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
    shown = reloadShown(await reload(live));
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
