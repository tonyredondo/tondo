// What the extension says of itself: a status bar item with the version
// of `tondo` and a menu of what it does, a command that makes a project,
// and where to get `tondo` when it is missing.

import * as os from 'node:os';
import * as path from 'node:path';
import * as vscode from 'vscode';
import { exec } from './cli';
import { incompatible } from './report';

/** How to get `tondo`, which the extension needs. */
export const INSTALL =
  'https://github.com/tonyredondo/tondo#getting-started';

export function registerStatus(context: vscode.ExtensionContext, output: vscode.LogOutputChannel): void {
  const item = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 0);
  context.subscriptions.push(
    item,
    vscode.commands.registerCommand('tondo.menu', menu),
    vscode.commands.registerCommand('tondo.newProject', newProject),
    vscode.commands.registerCommand('tondo.install', () => vscode.env.openExternal(vscode.Uri.parse(INSTALL))),
    vscode.commands.registerCommand('tondo.showOutput', () => output.show()),
    vscode.commands.registerCommand('tondo.walkthrough', () =>
      vscode.commands.executeCommand('workbench.action.openWalkthrough', 'tondo.tondo#start', false),
    ),
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (event.affectsConfiguration('tondo.path')) {
        void show(item);
      }
    }),
  );
  void show(item);
}

/** The version of `tondo`, or that it is missing or too old. */
async function show(item: vscode.StatusBarItem): Promise<void> {
  try {
    const ran = await exec(['version', '--json'], os.tmpdir());
    const problem = incompatible(ran.stdout);
    const version = JSON.parse(ran.stdout || '{}') as { version?: string; target?: string };
    item.text = problem ? '$(warning) Tondo' : `$(symbol-namespace) Tondo ${version.version ?? ''}`;
    item.tooltip = problem ?? `tondo ${version.version} for ${version.target}: the Tondo menu`;
    item.command = 'tondo.menu';
    void vscode.commands.executeCommand('setContext', 'tondo.installed', true);
  } catch (error) {
    // Not installed, or not where `tondo.path` says: why, and the setting.
    const configured = vscode.workspace.getConfiguration('tondo').get<string>('path', '').trim();
    item.text = '$(warning) Tondo';
    item.tooltip = configured ? (error as Error).message : 'tondo is not installed: how to get it';
    item.command = configured ? { title: 'Settings', command: 'workbench.action.openSettings', arguments: ['tondo.path'] } : 'tondo.install';
  }
  item.show();
}

async function menu(): Promise<void> {
  const choices = [
    { label: '$(new-folder) New Project', command: 'tondo.newProject' },
    { label: '$(beaker) Run Tests', command: 'testing.runAll' },
    { label: '$(type-hierarchy) Show the Module Graph', command: 'tondo.moduleGraph' },
    { label: '$(pulse) Watch a Running Program', command: 'tondo.watchCounters' },
    { label: '$(debug) Debug a Dump', command: 'tondo.debugDump' },
    { label: '$(flame) Show a Profile', command: 'tondo.showProfile' },
    { label: '$(refresh) Restart the Language Server', command: 'tondo.restartLanguageServer' },
    { label: '$(output) Show the Output', command: 'tondo.showOutput' },
    { label: '$(book) Get Started', command: 'tondo.walkthrough' },
  ];
  const chosen = await vscode.window.showQuickPick(choices, { placeHolder: 'Tondo' });
  if (chosen) {
    await vscode.commands.executeCommand(chosen.command);
  }
}

/** A project, with a program and a test, where the user says, opened. */
async function newProject(): Promise<void> {
  const name = await vscode.window.showInputBox({
    prompt: "The project's name",
    placeHolder: 'hello',
    validateInput: (value) => (/^[A-Za-z_][A-Za-z0-9_]*$/.test(value) ? undefined : 'A name is an identifier, as my_app'),
  });
  if (!name) {
    return;
  }
  const parent = await vscode.window.showOpenDialog({ canSelectFolders: true, canSelectFiles: false, openLabel: 'Make the project here' });
  if (!parent?.[0]) {
    return;
  }
  const dir = path.join(parent[0].fsPath, name);
  const ran = await exec(['new', dir, '--json'], parent[0].fsPath);
  if (ran.code !== 0) {
    void vscode.window.showErrorMessage(ran.stderr.trim() || ran.stdout.trim() || 'tondo new failed');
    return;
  }
  const open = await vscode.window.showInformationMessage(`Made the project ${name}.`, 'Open It', 'Open It in a New Window');
  if (open) {
    await vscode.commands.executeCommand('vscode.openFolder', vscode.Uri.file(dir), { forceNewWindow: open !== 'Open It' });
  }
}
