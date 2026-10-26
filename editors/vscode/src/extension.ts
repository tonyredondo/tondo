// Tondo in VS Code: its language server, debugger, tests, tasks and tools
// for agents, all served by the `tondo` executable.

import * as vscode from 'vscode';
import { LanguageClient, LanguageClientOptions, ServerOptions } from 'vscode-languageclient/node';
import { findTondo } from './binary';
import { registerDebugging } from './debugging';
import { registerTasks } from './tasks';
import { registerTesting } from './testing';

let client: LanguageClient | undefined;

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
  await startClient(output);
  registerDebugging(context);
  registerTesting(context, output);
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
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (event.affectsConfiguration('tondo.path')) {
        void restartClient(output);
      }
    }),
  );
}

export async function deactivate(): Promise<void> {
  await client?.stop();
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
  };
  client = new LanguageClient('tondo', 'Tondo', server, options);
  await client.start();
}

async function restartClient(output: vscode.LogOutputChannel): Promise<void> {
  await client?.stop();
  client = undefined;
  await startClient(output);
}
