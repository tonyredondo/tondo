// Tasks of `tondo`: check, build, run, test, fmt and watch, which runs a
// live build that takes in its files as they change (live.ts). Check and
// build read the diagnostics of `--json` into the Problems panel, with
// their ranges, and show them in the terminal.

import * as path from 'node:path';
import * as vscode from 'vscode';
import { isProject } from './binary';
import { exec, settings } from './cli';
import { tondo } from './extension';
import { checked, rendered } from './report';

export type Command = 'check' | 'build' | 'run' | 'test' | 'fmt' | 'watch';

interface Definition extends vscode.TaskDefinition {
  command: Command;
  target?: string;
}

/** The diagnostics of the last check or build of each project. */
let problems: vscode.DiagnosticCollection | undefined;

/** A task of `tondo` on a file or a project, run where it is; a program
 *  takes the arguments and the variables of the settings. */
export function tondoTask(command: Command, target: string, folder?: vscode.WorkspaceFolder): vscode.Task {
  const cwd = isProject(target) ? target : path.dirname(target);
  const program = settings('run', target);
  const args =
    command === 'watch'
      ? ['run', target, '--live', '--', ...program.args]
      : command === 'run'
        ? ['run', target, '--', ...program.args]
        : [command, target];
  const env = command === 'run' || command === 'watch' ? program.env : command === 'test' ? settings('test', target).env : {};
  const definition: Definition = { type: 'tondo', command, target };
  const name = `${command === 'watch' ? 'run live' : command} ${path.basename(target)}`;
  const execution =
    command === 'check' || command === 'build'
      ? diagnosed(command, target, cwd)
      : new vscode.ProcessExecution(tondo(), args, { cwd, env });
  const task = new vscode.Task(definition, folder ?? vscode.TaskScope.Workspace, name, 'tondo', execution);
  if (command === 'build' || command === 'check') {
    task.group = vscode.TaskGroup.Build;
  } else if (command === 'test') {
    task.group = vscode.TaskGroup.Test;
    task.problemMatchers = ['$tondo'];
  }
  if (command === 'run' || command === 'watch') {
    task.problemMatchers = ['$tondo'];
    task.presentationOptions = { focus: true, panel: vscode.TaskPanelKind.Dedicated, clear: true };
  }
  return task;
}

/** `tondo check` or `tondo build` with `--json`: its diagnostics in the
 *  Problems panel, in place of those of its last run, and in the
 *  terminal. */
function diagnosed(command: 'check' | 'build', target: string, cwd: string): vscode.CustomExecution {
  return new vscode.CustomExecution(async () => {
    const write = new vscode.EventEmitter<string>();
    const close = new vscode.EventEmitter<number>();
    const cancel = new vscode.CancellationTokenSource();
    const say = (text: string) => write.fire(text.replace(/\n/g, '\r\n') + '\r\n');
    const run = async () => {
      say(`tondo ${command} ${target}`);
      const ran = await exec([command, target, '--json'], cwd, cancel.token);
      let report;
      try {
        report = checked(ran.stdout);
      } catch {
        say(ran.stderr.trim() || `tondo ${command} failed`);
        return 1;
      }
      const found = new Map<string, vscode.Diagnostic[]>();
      for (const problem of report.problems) {
        say(rendered(problem));
        if (problem.file) {
          const file = path.resolve(cwd, problem.file);
          const range = new vscode.Range(problem.start.line, problem.start.column, problem.end.line, problem.end.column);
          const severity = problem.severity === 'error' ? vscode.DiagnosticSeverity.Error : vscode.DiagnosticSeverity.Warning;
          const diagnostic = new vscode.Diagnostic(range, problem.message, severity);
          diagnostic.source = 'tondo';
          diagnostic.code = problem.code || undefined;
          found.set(file, [...(found.get(file) ?? []), diagnostic]);
        }
      }
      problems?.forEach((uri) => {
        if (uri.fsPath.startsWith(cwd + path.sep)) {
          problems?.delete(uri);
        }
      });
      found.forEach((list, file) => problems?.set(vscode.Uri.file(file), list));
      const errors = report.problems.filter((p) => p.severity === 'error').length;
      say(report.ok ? (report.output ? `built ${path.resolve(cwd, report.output)}` : 'no errors') : `${errors} error${errors === 1 ? '' : 's'}`);
      return report.ok ? 0 : 1;
    };
    return {
      onDidWrite: write.event,
      onDidClose: close.event,
      open: () =>
        void run().then(
          (code) => close.fire(code),
          (error: Error) => {
            say(error.message);
            close.fire(1);
          },
        ),
      close: () => cancel.cancel(),
    };
  });
}

export function registerTasks(context: vscode.ExtensionContext): void {
  problems = vscode.languages.createDiagnosticCollection('tondo-tasks');
  context.subscriptions.push(
    problems,
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
