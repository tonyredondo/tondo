// Tondo in VS Code: its language server, debugger, tests, tasks and tools
// for agents, all served by the `tondo` executable.

import * as vscode from 'vscode';
import { LanguageClient, LanguageClientOptions, ServerOptions } from 'vscode-languageclient/node';
import { findTondo } from './binary';
import { exec } from './cli';
import { registerDebugging } from './debugging';
import { registerModules } from './modules';
import { registerObserving } from './observe';
import { registerTasks } from './tasks';
import { incompatible } from './report';
import { Served } from './report';
import { registerTesting } from './testing';

let client: LanguageClient | undefined;
/** Takes the tests the language server sends. */
let onTests: (params: Served) => void = () => undefined;

/** The `tondo` executable, or an error that says how to get it. */
export function tondo(): string {
  const configured = vscode.workspace.getConfiguration('tondo').get<string>('path', '');
  const found = findTondo(configured);
  if (!found) {
    throw new Error(
      'Tondo is not installed: run `sh dist/install.sh` from its repository, or set `tondo.path` to its executable.',
    );
  }
  return found;
}

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  const output = vscode.window.createOutputChannel('Tondo', { log: true });
  context.subscriptions.push(output);
  void checkVersion();
  onTests = registerTesting(context, output);
  await startClient(output);
  registerDebugging(context);
  registerModules(context);
  registerObserving(context, output);
  void vscode.commands.executeCommand('setContext', 'tondo.active', true);
  registerTasks(context);
  // The tools of `tondo mcp`, for VS Code's agents.
  context.subscriptions.push(
    vscode.lm.registerMcpServerDefinitionProvider('tondo', {
      provideMcpServerDefinitions: () => {
        try {
          return [new vscode.McpStdioServerDefinition('Tondo', tondo(), ['mcp'])];
        } catch {
          return [];
        }
      },
    }),
    vscode.commands.registerCommand('tondo.restartLanguageServer', () => restartClient(output)),
    // What a diagnostic's code means, which its link opens.
    vscode.workspace.registerTextDocumentContentProvider('tondo-explain', {
      provideTextDocumentContent: async (uri) => {
        const ran = await exec(['explain', uri.path], process.cwd());
        return ran.stdout || ran.stderr;
      },
    }),
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (event.affectsConfiguration('tondo.path')) {
        void checkVersion();
        void restartClient(output);
      }
    }),
  );
}

export async function deactivate(): Promise<void> {
  await client?.stop();
}

/** Says so when the `tondo` found is older than this extension. */
async function checkVersion(): Promise<void> {
  try {
    const ran = await exec(['version', '--json'], process.cwd());
    const problem = incompatible(ran.stdout);
    if (problem) {
      void vscode.window.showWarningMessage(problem);
    }
  } catch {
    // Without tondo, starting the language server says how to get it.
  }
}

async function startClient(output: vscode.LogOutputChannel): Promise<void> {
  let command: string;
  try {
    command = tondo();
  } catch (error) {
    void vscode.window.showErrorMessage((error as Error).message);
    return;
  }
  const server: ServerOptions = { command, args: ['lsp'] };
  const options: LanguageClientOptions = {
    documentSelector: [
      { scheme: 'file', language: 'tondo' },
      { scheme: 'untitled', language: 'tondo' },
    ],
    outputChannel: output,
    // Each project's tests as its files change (`tondo/tests`).
    initializationOptions: { tests: true },
  };
  client = new LanguageClient('tondo', 'Tondo', server, options);
  client.onNotification('tondo/tests', (params: Served) => onTests(params));
  await client.start();
}

async function restartClient(output: vscode.LogOutputChannel): Promise<void> {
  await client?.stop();
  client = undefined;
  await startClient(output);
}
