// Running and debugging with `tondo dap`, live builds included, whose
// code changes as their files are saved (live.ts). The lenses over `main`
// come from the language server, with these commands.

import * as path from 'node:path';
import * as vscode from 'vscode';
import { programFor, settings } from './cli';
import { tondo } from './extension';
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
