// Running and debugging with `tondo dap`, live builds whose code changes
// as their files are saved, and the lenses over `main`.

import * as path from 'node:path';
import * as vscode from 'vscode';
import { programOf } from './binary';
import { tondo } from './extension';
import { tondoTask } from './tasks';

export function registerDebugging(context: vscode.ExtensionContext): void {
  context.subscriptions.push(
    vscode.debug.registerDebugAdapterDescriptorFactory('tondo', {
      createDebugAdapterDescriptor: () => new vscode.DebugAdapterExecutable(tondo(), ['dap']),
    }),
    vscode.debug.registerDebugConfigurationProvider('tondo', {
      resolveDebugConfiguration: (folder, config) => {
        // F5 without a launch.json: the program of the file open.
        if (!config.type && !config.request && !config.name) {
          const file = activeFile();
          if (!file) {
            void vscode.window.showInformationMessage('Open a Tondo file to debug its program.');
            return undefined;
          }
          return { type: 'tondo', request: 'launch', name: 'Debug', ...programOf(file) };
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
    vscode.commands.registerCommand('tondo.reload', () => reload(vscode.debug.activeDebugSession, true)),
    // Edit and Continue: a live build debugged takes in what is saved.
    vscode.workspace.onDidSaveTextDocument((document) => {
      const session = vscode.debug.activeDebugSession;
      const enabled = vscode.workspace.getConfiguration('tondo').get<boolean>('reloadOnSave', true);
      if (enabled && document.languageId === 'tondo' && session?.type === 'tondo' && session.configuration.live) {
        void reload(session, false);
      }
    }),
    vscode.languages.registerCodeLensProvider({ language: 'tondo' }, { provideCodeLenses: lenses }),
  );
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
    await vscode.tasks.executeTask(tondoTask(command, programOf(file).program, folder));
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
  await vscode.debug.startDebugging(folder, { type: 'tondo', request: 'launch', name, live, ...programOf(file) });
}

/** Puts in the code of the files as they are now, and says what changed. */
async function reload(session: vscode.DebugSession | undefined, asked: boolean): Promise<void> {
  if (session?.type !== 'tondo') {
    if (asked) {
      void vscode.window.showInformationMessage('Reload changes a live build while it is debugged: start one with Tondo: Debug Live.');
    }
    return;
  }
  try {
    const reloaded = (await session.customRequest('reload')) as { changed?: string[]; added?: string[] };
    const changed = reloaded.changed ?? [];
    const added = reloaded.added ?? [];
    const what = [
      changed.length ? `changed ${changed.join(', ')}` : '',
      added.length ? `added ${added.join(', ')}` : '',
    ].filter((part) => part !== '');
    vscode.window.setStatusBarMessage(`Tondo: reloaded${what.length ? ': ' + what.join('; ') : ''}`, 5000);
  } catch (error) {
    void vscode.window.showWarningMessage(`Tondo: ${(error as Error).message}`);
  }
}

/** Run, debug and debug live over `fn main`. */
function lenses(document: vscode.TextDocument): vscode.CodeLens[] {
  const found: vscode.CodeLens[] = [];
  for (let line = 0; line < document.lineCount; line++) {
    if (!/^(pub\s+)?fn\s+main\s*\(/.test(document.lineAt(line).text)) {
      continue;
    }
    const range = document.lineAt(line).range;
    found.push(
      new vscode.CodeLens(range, { title: 'Run', command: 'tondo.run', arguments: [document.uri] }),
      new vscode.CodeLens(range, { title: 'Debug', command: 'tondo.debug', arguments: [document.uri] }),
      new vscode.CodeLens(range, { title: 'Debug Live', command: 'tondo.debugLive', arguments: [document.uri] }),
    );
  }
  return found;
}
