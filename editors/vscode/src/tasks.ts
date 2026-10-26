// Tasks of `tondo`: check, build, run, test, fmt and watch, with its
// diagnostics in the Problems panel.

import * as path from 'node:path';
import * as vscode from 'vscode';
import { isProject } from './binary';
import { tondo } from './extension';

export type Command = 'check' | 'build' | 'run' | 'test' | 'fmt' | 'watch';

interface Definition extends vscode.TaskDefinition {
  command: Command;
  target?: string;
}

/** A task of `tondo` on a file or a project, run where it is. */
export function tondoTask(command: Command, target: string, folder?: vscode.WorkspaceFolder): vscode.Task {
  const cwd = isProject(target) ? target : path.dirname(target);
  const args = command === 'watch' ? ['run', target, '--watch'] : [command, target];
  const definition: Definition = { type: 'tondo', command, target };
  const name = `${command === 'watch' ? 'run live' : command} ${path.basename(target)}`;
  const task = new vscode.Task(
    definition,
    folder ?? vscode.TaskScope.Workspace,
    name,
    'tondo',
    new vscode.ProcessExecution(tondo(), args, { cwd }),
    ['$tondo'],
  );
  if (command === 'build' || command === 'check') {
    task.group = vscode.TaskGroup.Build;
  } else if (command === 'test') {
    task.group = vscode.TaskGroup.Test;
  }
  if (command === 'run' || command === 'watch') {
    task.presentationOptions = { focus: true, panel: vscode.TaskPanelKind.Dedicated, clear: true };
  }
  return task;
}

export function registerTasks(context: vscode.ExtensionContext): void {
  context.subscriptions.push(
    vscode.tasks.registerTaskProvider('tondo', {
      provideTasks: () => {
        const tasks: vscode.Task[] = [];
        for (const folder of vscode.workspace.workspaceFolders ?? []) {
          if (!isProject(folder.uri.fsPath)) {
            continue;
          }
          for (const command of ['check', 'build', 'run', 'test', 'watch'] as Command[]) {
            try {
              tasks.push(tondoTask(command, folder.uri.fsPath, folder));
            } catch {
              return [];
            }
          }
        }
        return tasks;
      },
      resolveTask: (task) => {
        const definition = task.definition as Definition;
        const folder = typeof task.scope === 'object' ? task.scope : undefined;
        const target = definition.target ?? folder?.uri.fsPath;
        return target ? tondoTask(definition.command, target, folder) : undefined;
      },
    }),
  );
}
