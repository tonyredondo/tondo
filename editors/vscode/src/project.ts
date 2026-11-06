// What the command line does for a project, from the command palette: the
// guide and the API, what a build expanded or leaves unreached, dependencies
// and caches; and any other command, with its options, in a terminal.

import * as path from 'node:path';
import * as vscode from 'vscode';
import { exec, placeOf } from './cli';
import { tondo } from './extension';
import { apiMatches, dependencyArgs, splitArgs, unreachedMarks } from './report';

/** The project of the file in view, or the first folder of the workspace. */
async function projectDir(): Promise<string | undefined> {
  const file = vscode.window.activeTextEditor?.document.uri;
  if (file?.scheme === 'file' && file.fsPath.endsWith('.to')) {
    const place = await placeOf(file.fsPath).catch(() => undefined);
    return place?.project ? place.root : path.dirname(file.fsPath);
  }
  return vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
}

/** A text beside the code, as `language`. */
async function show(content: string, language: string): Promise<void> {
  const document = await vscode.workspace.openTextDocument({ content, language });
  await vscode.window.showTextDocument(document, vscode.ViewColumn.Beside);
}

/** Runs `tondo` in a terminal of its own, which shows what it says. */
async function inTerminal(args: string[], cwd: string): Promise<void> {
  const execution = new vscode.ProcessExecution(tondo(), args, { cwd });
  const name = `tondo ${args.join(' ')}`;
  const task = new vscode.Task({ type: 'tondo', command: args[0] }, vscode.TaskScope.Workspace, name, 'tondo', execution);
  task.presentationOptions = { focus: true, panel: vscode.TaskPanelKind.Dedicated, clear: true };
  await vscode.tasks.executeTask(task);
}

/** What `tondo` printed, or its failure said to the user. */
async function said(args: string[], cwd: string): Promise<string | undefined> {
  const ran = await exec(args, cwd);
  if (ran.code !== 0) {
    void vscode.window.showErrorMessage(ran.stderr.trim() || `tondo ${args[0]} failed`);
    return undefined;
  }
  return ran.stdout;
}

const asked = (prompt: string, value = '') => vscode.window.showInputBox({ prompt, value });

/** A topic of the guide, picked, as Markdown. */
async function guide(): Promise<void> {
  const cwd = (await projectDir()) ?? '.';
  const list = await said(['guide', '--list', '--json'], cwd);
  const topic = list && (await vscode.window.showQuickPick((JSON.parse(list) as { topics: string[] }).topics));
  const text = topic && (await said(['guide', topic], cwd));
  if (text) {
    await show(text, 'markdown');
  }
}

/** What a module of the project or the standard library declares. */
async function showApi(extra: string[]): Promise<void> {
  const cwd = await projectDir();
  if (!cwd) {
    return;
  }
  const list = await said(['api', '--list', '--project', cwd, '--json'], cwd);
  const module = list && (await vscode.window.showQuickPick((JSON.parse(list) as { modules: string[] }).modules));
  const text = module && (await said(['api', module, '--project', cwd, ...extra], cwd));
  if (text) {
    await show(text, 'tondo');
  }
}

/** The declarations that some words find, and the one picked. */
async function searchApi(): Promise<void> {
  const cwd = await projectDir();
  const words = cwd && (await asked('What to find, in names and documentation'));
  if (!cwd || !words) {
    return;
  }
  const found = await said(['api', '--search', words, '--project', cwd, '--json'], cwd);
  const picked = found && (await vscode.window.showQuickPick(apiMatches(found), { matchOnDetail: true }));
  const text = picked && (await said(['api', `${picked.description}.${picked.label}`, '--project', cwd], cwd));
  if (text) {
    await show(text, 'tondo');
  }
}

/** What is not reached by `main` or a test. */
const unreached = vscode.languages.createDiagnosticCollection('tondo-unreached');

async function markUnreached(): Promise<void> {
  const cwd = await projectDir();
  const found = cwd && (await said(['check', cwd, '--unreached', '--json'], cwd));
  if (!cwd || !found) {
    return;
  }
  unreached.clear();
  const byFile = new Map<string, vscode.Diagnostic[]>();
  const marks = unreachedMarks(found);
  for (const { file, line, text } of marks) {
    const mark = new vscode.Diagnostic(new vscode.Range(line - 1, 0, line - 1, Number.MAX_SAFE_INTEGER), text, vscode.DiagnosticSeverity.Hint);
    mark.source = 'tondo check';
    mark.tags = [vscode.DiagnosticTag.Unnecessary];
    byFile.set(file, [...(byFile.get(file) ?? []), mark]);
  }
  for (const [file, list] of byFile) {
    unreached.set(vscode.Uri.file(path.resolve(cwd, file)), list);
  }
  void vscode.window.showInformationMessage(`${marks.length} declarations nothing reaches, in Problems.`);
}

/** A dependency, from a directory, a repository or an archive. */
async function addDependency(): Promise<void> {
  const cwd = await projectDir();
  const kind = cwd && (await vscode.window.showQuickPick(['path', 'git', 'url'], { title: 'Where it comes from' }));
  const where = kind && (await asked(`Its ${kind}`));
  const alias = where && (await asked('The name to import it by'));
  if (cwd && kind && where && alias) {
    await inTerminal(dependencyArgs(alias, kind, where, cwd), cwd);
  }
}

async function compact(): Promise<void> {
  const file = vscode.window.activeTextEditor?.document;
  const text = file?.fileName.endsWith('.to') && (await said(['fmt', '--compact', file.fileName], path.dirname(file.fileName)));
  if (text) {
    await show(text, 'tondo');
  }
}

async function expansions(): Promise<void> {
  const cwd = await projectDir();
  const text = cwd && (await said(['expand', cwd], cwd));
  if (text) {
    await show(text, 'tondo');
  } else if (text === '') {
    void vscode.window.showInformationMessage('Nothing here is derived.');
  }
}

async function exportGraph(): Promise<void> {
  const cwd = await projectDir();
  const format = cwd && (await vscode.window.showQuickPick(['dot', 'mermaid'], { title: 'As' }));
  const text = cwd && format && (await said(['graph', cwd, `--${format}`], cwd));
  if (text) {
    await show(text, format === 'dot' ? 'dot' : 'markdown');
  }
}

/** The file for lldb or gdb, and how to load it. */
async function debuggerPlugin(): Promise<void> {
  const debuggerName = await vscode.window.showQuickPick(['lldb', 'gdb']);
  const text = debuggerName && (await said(['dump', 'plugin', debuggerName, '--json'], '.'));
  if (text) {
    const { load } = JSON.parse(text) as { load: string };
    await vscode.env.clipboard.writeText(load);
    void vscode.window.showInformationMessage(`Copied, to run in ${debuggerName}: ${load}`);
  }
}

/** In a terminal, with the project as it is: the ask is the command. */
const inProject = (args: (dir: string) => Promise<string[] | undefined>) => async () => {
  const cwd = await projectDir();
  const written = cwd && (await args(cwd));
  if (cwd && written) {
    await inTerminal(written, cwd);
  }
};

export function registerProject(context: vscode.ExtensionContext): void {
  let last = '';
  context.subscriptions.push(
    unreached,
    vscode.commands.registerCommand('tondo.guide', guide),
    vscode.commands.registerCommand('tondo.showApi', () => showApi([])),
    vscode.commands.registerCommand('tondo.showPrivateApi', () => showApi(['--private'])),
    vscode.commands.registerCommand('tondo.searchApi', searchApi),
    vscode.commands.registerCommand('tondo.expand', expansions),
    vscode.commands.registerCommand('tondo.compact', compact),
    vscode.commands.registerCommand('tondo.unreached', markUnreached),
    vscode.commands.registerCommand('tondo.exportGraph', exportGraph),
    vscode.commands.registerCommand('tondo.addDependency', addDependency),
    vscode.commands.registerCommand('tondo.dumpPlugin', debuggerPlugin),
    vscode.commands.registerCommand(
      'tondo.removeDependency',
      inProject(async (dir) => {
        const alias = await asked('The dependency to take out');
        return alias ? ['remove', alias, '--project', dir] : undefined;
      }),
    ),
    vscode.commands.registerCommand(
      'tondo.updateDependencies',
      inProject(async (dir) => ['update', dir]),
    ),
    vscode.commands.registerCommand(
      'tondo.clean',
      inProject(async () => {
        const what = await vscode.window.showQuickPick(['The cache of builds', 'And the fetched dependencies']);
        return what ? ['clean', ...(what.startsWith('And') ? ['--all'] : [])] : undefined;
      }),
    ),
    vscode.commands.registerCommand(
      'tondo.initProject',
      inProject(async (dir) => {
        const name = await asked('The name of the project', path.basename(dir));
        return name ? ['init', dir, '--name', name] : undefined;
      }),
    ),
    vscode.commands.registerCommand(
      'tondo.buildLibrary',
      inProject(async (dir) => ['build', '--library', dir]),
    ),
    vscode.commands.registerCommand(
      'tondo.bind',
      inProject(async () => {
        const written = await asked('The C headers and the options of `tondo bind`');
        return written ? ['bind', ...splitArgs(written)] : undefined;
      }),
    ),
    vscode.commands.registerCommand(
      'tondo.targetPack',
      inProject(async () => {
        const dir = await asked('The directory of packs; empty for the usual one');
        return dir === undefined ? undefined : ['target', 'pack', ...(dir ? [dir] : [])];
      }),
    ),
    vscode.commands.registerCommand(
      'tondo.runCommand',
      inProject(async () => {
        const written = await asked('After `tondo`: any command, with its options', last);
        last = written ?? last;
        return written ? splitArgs(written) : undefined;
      }),
    ),
  );
}
