// Running and debugging with `tondo dap`, live builds included, whose
// code changes as their files are saved (live.ts), and a value's parts as
// a table. The lenses over `main` come from the language server, with
// these commands.

import { randomBytes } from 'node:crypto';
import * as path from 'node:path';
import * as vscode from 'vscode';
import { programFor, settings } from './cli';
import { tondo } from './extension';
import { Diff, Origin, Table, Task, Timeline, diffMarkdown, mermaid, originsMarkdown, shortPlace, tableHtml, timelineHtml } from './report';
import { tondoTask } from './tasks';

export function registerDebugging(context: vscode.ExtensionContext): void {
  context.subscriptions.push(
    vscode.debug.registerDebugAdapterDescriptorFactory('tondo', {
      createDebugAdapterDescriptor: () => new vscode.DebugAdapterExecutable(tondo(), ['dap']),
    }),
    vscode.debug.registerDebugConfigurationProvider('tondo', {
      resolveDebugConfiguration: async (folder, config) => {
        // F5 without a launch.json: the program of the file open.
        if (!config.type && !config.request && !config.name) {
          const file = activeFile();
          if (!file) {
            void vscode.window.showInformationMessage('Open a Tondo file to debug its program.');
            return undefined;
          }
          return { type: 'tondo', request: 'launch', name: 'Debug', ...(await launched(file)) };
        }
        if (config.request === 'launch' && !config.cwd) {
          config.cwd = folder?.uri.fsPath ?? path.dirname(String(config.program ?? ''));
        }
        return config;
      },
    }),
    vscode.commands.registerCommand('tondo.run', (uri?: vscode.Uri) => run(uri, 'run')),
    vscode.commands.registerCommand('tondo.runLive', (uri?: vscode.Uri) => run(uri, 'watch')),
    vscode.commands.registerCommand('tondo.debug', (uri?: vscode.Uri) => debug(uri, false)),
    vscode.commands.registerCommand('tondo.debugLive', (uri?: vscode.Uri) => debug(uri, true)),
    vscode.commands.registerCommand('tondo.showTable', (context?: { variable?: Variable }) => showTable(context?.variable)),
    vscode.commands.registerCommand('tondo.why', (context?: { variable?: Variable }) =>
      why(context?.variable ? (context.variable.evaluateName ?? context.variable.name) : selected()),
    ),
    vscode.languages.registerInlineValuesProvider({ language: 'tondo' }, { provideInlineValues: effects }),
    vscode.commands.registerCommand('tondo.taskGraph', taskGraph),
    vscode.commands.registerCommand('tondo.when', when),
    vscode.commands.registerCommand('tondo.diff', diff),
    vscode.commands.registerCommand('tondo.origins', (context?: { variable?: Variable }) =>
      origins(context?.variable ? (context.variable.evaluateName ?? context.variable.name) : selected()),
    ),
    vscode.commands.registerCommand('tondo.timeline', (context?: { variable?: Variable }) =>
      timeline(context?.variable ? (context.variable.evaluateName ?? context.variable.name) : selected()),
    ),
  );
}

/** What changed in a replay between another probe and where the program
 *  is, beside the code. */
async function diff(): Promise<void> {
  const session = vscode.debug.activeDebugSession;
  if (session?.type !== 'tondo') {
    void vscode.window.showInformationMessage('Replay a recorded run to compare its moments: tondo run --record <dir>.');
    return;
  }
  const from = await vscode.window.showInputBox({
    prompt: 'The probe to compare with, as Show Its Values over the Run and Where Does It Come From? give them',
    validateInput: (text) => (/^\d+$/.test(text.trim()) ? undefined : 'a probe is a number'),
  });
  if (!from) {
    return;
  }
  try {
    const found = (await session.customRequest('diff', { from: Number(from.trim()) })) as { diff: Diff };
    const document = await vscode.workspace.openTextDocument({ content: diffMarkdown(found.diff), language: 'markdown' });
    await vscode.window.showTextDocument(document, vscode.ViewColumn.Beside);
  } catch (error) {
    void vscode.window.showErrorMessage((error as Error).message);
  }
}

/** In a replay, to the line after which a condition held, the first or the
 *  last time over the whole run. */
async function when(): Promise<void> {
  const session = vscode.debug.activeDebugSession;
  if (session?.type !== 'tondo') {
    void vscode.window.showInformationMessage('Replay a recorded run to search it: tondo run --record <dir>.');
    return;
  }
  const condition = await vscode.window.showInputBox({ prompt: 'A condition, as total > 100', value: selected() });
  if (!condition) {
    return;
  }
  const which = await vscode.window.showQuickPick(['first', 'last'], { placeHolder: 'The first or the last time it holds' });
  if (!which) {
    return;
  }
  const item = vscode.debug.activeStackItem;
  const frameId = item instanceof vscode.DebugStackFrame && item.session.id === session.id ? item.frameId : undefined;
  try {
    await session.customRequest('when', { condition, which, frameId });
  } catch (error) {
    void vscode.window.showErrorMessage((error as Error).message);
  }
}

/** Where a value came from in a replay, back over the run: the line that
 *  gave it, what that line read, and so on, beside the code. */
async function origins(expression?: string): Promise<void> {
  const session = vscode.debug.activeDebugSession;
  if (session?.type !== 'tondo' || !expression) {
    void vscode.window.showInformationMessage('Select a binding of the program, or one in the Variables view.');
    return;
  }
  const item = vscode.debug.activeStackItem;
  const frameId = item instanceof vscode.DebugStackFrame && item.session.id === session.id ? item.frameId : undefined;
  let found: { origins: { why: string; at: string; causes: Origin[] } };
  try {
    found = (await session.customRequest('origins', { expression, frameId })) as typeof found;
  } catch (error) {
    void vscode.window.showErrorMessage((error as Error).message);
    return;
  }
  const document = await vscode.workspace.openTextDocument({ content: originsMarkdown(expression, found.origins), language: 'markdown' });
  await vscode.window.showTextDocument(document, vscode.ViewColumn.Beside);
}

/** Each value an expression took over the run beside the code: in a
 *  replay, on a bar of the run whose changes take the program to them. */
async function timeline(expression?: string): Promise<void> {
  const session = vscode.debug.activeDebugSession;
  if (session?.type !== 'tondo' || !expression) {
    void vscode.window.showInformationMessage('Select a binding of the program, or one in the Variables view.');
    return;
  }
  const item = vscode.debug.activeStackItem;
  const frameId = item instanceof vscode.DebugStackFrame && item.session.id === session.id ? item.frameId : undefined;
  let found: Timeline;
  try {
    found = (await session.customRequest('timeline', { expression, frameId })) as Timeline;
  } catch (error) {
    void vscode.window.showErrorMessage((error as Error).message);
    return;
  }
  const panel = vscode.window.createWebviewPanel('tondoTimeline', expression, vscode.ViewColumn.Beside, { enableScripts: true });
  panel.webview.html = timelineHtml(expression, found, randomBytes(16).toString('hex'));
  panel.webview.onDidReceiveMessage(async (asked: { seek: number }) => {
    try {
      await session.customRequest('seek', { tick: asked.seek });
      await panel.webview.postMessage({ tick: asked.seek });
    } catch (error) {
      void vscode.window.showErrorMessage((error as Error).message);
    }
  });
}

/** The tasks of the program stopped, as a graph: each task where it is,
 *  and an arrow to each task that holds what it waits for. */
async function taskGraph(): Promise<void> {
  const session = vscode.debug.activeDebugSession;
  if (session?.type !== 'tondo') {
    void vscode.window.showInformationMessage('Stop a Tondo program in the debugger to see what its tasks wait for.');
    return;
  }
  let tasks: Task[];
  try {
    tasks = ((await session.customRequest('tasks')) as { tasks: Task[] }).tasks;
  } catch (error) {
    void vscode.window.showErrorMessage((error as Error).message);
    return;
  }
  const content = `# The tasks of ${session.name}\n\n\`\`\`mermaid\n${mermaid(tasks)}\`\`\`\n`;
  const document = await vscode.workspace.openTextDocument({ content, language: 'markdown' });
  await vscode.window.showTextDocument(document, vscode.ViewColumn.Beside);
}


/** Beside each line of the call the debugger shows, what it changed the
 *  last time it ran, as the history `record` keeps has it. */
async function effects(
  document: vscode.TextDocument,
  _viewport: vscode.Range,
  context: vscode.InlineValueContext,
): Promise<vscode.InlineValue[]> {
  const session = vscode.debug.activeDebugSession;
  if (session?.type !== 'tondo') {
    return [];
  }
  let found: { lines: { path: string; line: number; text: string }[] };
  try {
    found = (await session.customRequest('effects', { frameId: context.frameId })) as typeof found;
  } catch {
    return [];
  }
  return found.lines
    .filter((line) => path.resolve(line.path) === document.uri.fsPath && line.line - 1 < document.lineCount)
    .map((line) => {
      const end = document.lineAt(line.line - 1).range.end;
      return new vscode.InlineValueText(new vscode.Range(end, end), `then ${line.text}`);
    });
}

/** What `why` answers: the line that gave the value, and where each
 *  binding that line reads got its own. */
interface Why {
  why: string;
  at: string;
  causes: { name: string; value: string; at: string }[];
}

/** The line that gave an expression its value: the program goes back to
 *  it, as it was in a replay, or in the history `record` keeps; then each
 *  binding that line reads may be asked about in turn. */
async function why(expression?: string): Promise<void> {
  const session = vscode.debug.activeDebugSession;
  if (session?.type !== 'tondo' || !expression) {
    void vscode.window.showInformationMessage('Select a binding of the program, or one in the Variables view.');
    return;
  }
  const item = vscode.debug.activeStackItem;
  const frameId = item instanceof vscode.DebugStackFrame && item.session.id === session.id ? item.frameId : undefined;
  let found: Why;
  try {
    found = (await session.customRequest('why', { expression, frameId })) as Why;
  } catch (error) {
    void vscode.window.showErrorMessage((error as Error).message);
    return;
  }
  const causes = found.causes.slice(0, 3);
  const from = causes.map((cause) => `\`${cause.name}\` was ${cause.value} at ${shortPlace(cause.at)}`).join('; ');
  const message = `${found.why}, at ${shortPlace(found.at)}${from ? `. ${from}` : ''}.`;
  const asked = await vscode.window.showInformationMessage(message, ...causes.map((cause) => `Why ${cause.name}?`));
  const cause = causes.find((cause) => `Why ${cause.name}?` === asked);
  if (cause) {
    await why(cause.name);
  }
}


/** The expression selected in the editor, or the word at the cursor. */
function selected(): string | undefined {
  const editor = vscode.window.activeTextEditor;
  if (!editor) {
    return undefined;
  }
  if (!editor.selection.isEmpty) {
    return editor.document.getText(editor.selection).trim();
  }
  const word = editor.document.getWordRangeAtPosition(editor.selection.active);
  return word ? editor.document.getText(word) : undefined;
}

/** A variable of the debugger's view, as its menu gives it. */
interface Variable {
  name: string;
  variablesReference: number;
  evaluateName?: string;
}

/** A value's parts as a table beside the code, as of the stop: an array
 *  or a map of records, a row for each part and a column for each field. */
async function showTable(variable?: Variable): Promise<void> {
  const session = vscode.debug.activeDebugSession;
  if (session?.type !== 'tondo' || !variable?.variablesReference) {
    void vscode.window.showInformationMessage('Only a value with parts, as an array or a map, shows as a table.');
    return;
  }
  const page = async (start: number, stop?: number) =>
    (await session.customRequest('table', { variablesReference: variable.variablesReference, start, count: 500, stop })) as Table;
  let table: Table;
  try {
    table = await page(0);
  } catch (error) {
    void vscode.window.showErrorMessage((error as Error).message);
    return;
  }
  const title = variable.evaluateName ?? variable.name;
  const panel = vscode.window.createWebviewPanel('tondoTable', title, vscode.ViewColumn.Beside, { enableScripts: true });
  panel.webview.html = tableHtml(title, table, randomBytes(16).toString('hex'));
  panel.webview.onDidReceiveMessage(async (asked: { start: number; stop: number }) => {
    try {
      await panel.webview.postMessage(await page(asked.start, asked.stop));
    } catch (error) {
      void vscode.window.showErrorMessage((error as Error).message);
    }
  });
}

/** The file a command is for: the one it was given, or the one open. */
function activeFile(uri?: vscode.Uri): string | undefined {
  if (uri?.scheme === 'file') {
    return uri.fsPath;
  }
  const document = vscode.window.activeTextEditor?.document;
  return document?.languageId === 'tondo' && document.uri.scheme === 'file' ? document.uri.fsPath : undefined;
}

async function run(uri: vscode.Uri | undefined, command: 'run' | 'watch'): Promise<void> {
  const file = activeFile(uri);
  if (!file) {
    void vscode.window.showInformationMessage('Open a Tondo file to run its program.');
    return;
  }
  await vscode.workspace.saveAll(false);
  const folder = vscode.workspace.getWorkspaceFolder(vscode.Uri.file(file));
  try {
    await vscode.tasks.executeTask(tondoTask(command, (await programFor(file)).program, folder));
  } catch (error) {
    void vscode.window.showErrorMessage((error as Error).message);
  }
}

async function debug(uri: vscode.Uri | undefined, live: boolean): Promise<void> {
  const file = activeFile(uri);
  if (!file) {
    void vscode.window.showInformationMessage('Open a Tondo file to debug its program.');
    return;
  }
  const folder = vscode.workspace.getWorkspaceFolder(vscode.Uri.file(file));
  const name = live ? 'Debug live' : 'Debug';
  try {
    await vscode.debug.startDebugging(folder, { type: 'tondo', request: 'launch', name, live, ...(await launched(file)) });
  } catch (error) {
    void vscode.window.showErrorMessage((error as Error).message);
  }
}

/** What to debug for a file, with the arguments and the variables of the
 *  settings. */
async function launched(file: string): Promise<{ program: string; cwd: string; args: string[]; env: Record<string, string> }> {
  return { ...(await programFor(file)), ...settings('run', file) };
}
