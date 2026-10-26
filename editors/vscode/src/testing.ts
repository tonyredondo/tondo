// The tests of the workspace's projects in the Testing view: listed by
// `tondo test --list`, run by `tondo test`, whose events show each result
// as it comes, and debugged by `tondo dap`.

import * as path from 'node:path';
import * as vscode from 'vscode';
import { isProject } from './binary';
import { Ran, exec, placeOf } from './cli';
import { Outcome, listed, testEvent } from './report';

export function registerTesting(context: vscode.ExtensionContext, output: vscode.OutputChannel): void {
  const controller = vscode.tests.createTestController('tondo', 'Tondo');
  context.subscriptions.push(controller);
  // Each project's item, and the project of each item.
  const projects = new Map<string, vscode.TestItem>();
  const projectOf = new WeakMap<vscode.TestItem, string>();

  async function discoverAll(): Promise<void> {
    const found = await vscode.workspace.findFiles('**/{tondo.toml,src/main.to}', '**/{target,node_modules,.git}/**');
    const dirs = new Set(
      found.map((file) => {
        const dir = path.dirname(file.fsPath);
        return path.basename(file.fsPath) === 'main.to' ? path.dirname(dir) : dir;
      }),
    );
    for (const [dir, item] of projects) {
      if (!dirs.has(dir)) {
        controller.items.delete(item.id);
        projects.delete(dir);
      }
    }
    await Promise.all([...dirs].map(discover));
  }

  async function discover(dir: string): Promise<void> {
    let project = projects.get(dir);
    if (!project) {
      project = controller.createTestItem(`project:${dir}`, path.basename(dir), vscode.Uri.file(dir));
      projects.set(dir, project);
      projectOf.set(project, dir);
      controller.items.add(project);
    }
    let ran: Ran;
    try {
      ran = await exec(['test', '--list', '--test-format', 'json'], dir);
    } catch (error) {
      project.error = (error as Error).message;
      return;
    }
    let nodes;
    try {
      nodes = listed(ran.stdout);
    } catch {
      project.error = ran.stderr.trim() || 'tondo test --list gave no list';
      return;
    }
    project.error = undefined;
    const items = new Map<string, vscode.TestItem>();
    for (const node of nodes) {
      const uri = node.file ? vscode.Uri.file(path.join(dir, node.file)) : undefined;
      const item = controller.createTestItem(node.id, node.name, uri);
      if (node.line) {
        const position = new vscode.Position(node.line - 1, Math.max((node.column ?? 1) - 1, 0));
        item.range = new vscode.Range(position, position);
      }
      projectOf.set(item, dir);
      items.set(node.id, item);
    }
    const roots: vscode.TestItem[] = [];
    for (const node of nodes) {
      const item = items.get(node.id)!;
      const parent = node.parent ? items.get(node.parent) : undefined;
      if (parent) {
        parent.children.add(item);
      } else {
        roots.push(item);
      }
    }
    project.children.replace(roots);
  }

  controller.resolveHandler = async (item) => {
    if (!item) {
      await discoverAll();
    }
  };
  controller.refreshHandler = () => discoverAll();

  // A file saved lists its project's tests again, and a new project's.
  const pending = new Map<string, NodeJS.Timeout>();
  context.subscriptions.push(
    vscode.workspace.onDidSaveTextDocument(async (document) => {
      if (document.languageId !== 'tondo' || document.uri.scheme !== 'file') {
        return;
      }
      const where = await placeOf(document.uri.fsPath).catch(() => undefined);
      if (where?.project) {
        clearTimeout(pending.get(where.root));
        pending.set(where.root, setTimeout(() => void discover(where.root), 500));
      }
    }),
  );

  /** The tests under an item, which a run starts. */
  function tests(item: vscode.TestItem, into: vscode.TestItem[]): vscode.TestItem[] {
    if (item.children.size === 0 && !item.id.startsWith('project:')) {
      into.push(item);
    }
    item.children.forEach((child) => tests(child, into));
    return into;
  }

  async function run(request: vscode.TestRunRequest, token: vscode.CancellationToken): Promise<void> {
    const testRun = controller.createTestRun(request);
    const included = request.include ?? [...projects.values()];
    const excluded = new Set(request.exclude ?? []);
    // What each project runs: everything, or the items asked for.
    const byProject = new Map<string, vscode.TestItem[]>();
    for (const item of included) {
      const dir = projectOf.get(item);
      if (dir && !excluded.has(item)) {
        byProject.set(dir, [...(byProject.get(dir) ?? []), item]);
      }
    }
    for (const [dir, items] of byProject) {
      const whole = items.some((item) => item.id.startsWith('project:'));
      const selections = whole ? [undefined] : items.map((item) => item.id);
      const started = items.flatMap((item) => tests(item, []));
      started.forEach((item) => testRun.enqueued(item));
      const known = new Map(started.map((item) => [item.id, item]));
      for (const selection of selections) {
        if (token.isCancellationRequested) {
          break;
        }
        const args = ['test', '--test-format', 'events', '--allow-empty'];
        if (selection) {
          args.push('--exact', selection);
        }
        let reported = false;
        const ended = (outcome: Outcome, duration?: number) => {
          const item = known.get(outcome.id);
          if (!item) {
            return;
          }
          const location =
            outcome.file && outcome.line
              ? new vscode.Location(
                  vscode.Uri.file(path.join(dir, outcome.file)),
                  new vscode.Position(outcome.line - 1, Math.max((outcome.column ?? 1) - 1, 0)),
                )
              : undefined;
          const message = new vscode.TestMessage(outcome.message ?? outcome.verdict);
          message.location = location;
          switch (outcome.verdict) {
            case 'passed':
              testRun.passed(item, duration);
              break;
            case 'skipped':
              testRun.skipped(item);
              break;
            case 'failed':
              testRun.failed(item, message, duration);
              break;
            case 'errored':
              testRun.errored(item, message, duration);
              break;
          }
          return location;
        };
        const ran: Ran = await exec(args, dir, token, (line) => {
          const event = testEvent(line);
          switch (event?.event) {
            case 'start':
              if (known.has(event.id)) {
                testRun.started(known.get(event.id)!);
              }
              break;
            case 'end': {
              // Each attempt as it ends; a retry may change it.
              const location = ended(event.outcome, event.duration);
              const item = known.get(event.outcome.id);
              if (item && event.outcome.output) {
                testRun.appendOutput(event.outcome.output.replace(/\r?\n/g, '\r\n'), location, item);
              }
              break;
            }
            case 'report':
              reported = event.compiled;
              // The report decides, after every retry.
              for (const outcome of event.outcomes) {
                ended(outcome);
                known.delete(outcome.id);
              }
              break;
          }
        }).catch((error: Error) => ({ code: null, stdout: '', stderr: error.message }));
        if (!reported) {
          // It did not compile, or tondo failed: why goes on every test.
          const message = new vscode.TestMessage(ran.stderr.trim() || 'tondo test gave no report');
          for (const item of started.filter((item) => !selection || item.id.startsWith(selection))) {
            testRun.errored(item, message);
            known.delete(item.id);
          }
          continue;
        }
        if (ran.stderr.trim()) {
          output.appendLine(ran.stderr.trimEnd());
        }
      }
      // What the report does not name did not run.
      known.forEach((item) => testRun.skipped(item));
    }
    testRun.end();
  }

  async function debug(request: vscode.TestRunRequest): Promise<void> {
    const chosen = (request.include ?? []).flatMap((item) => tests(item, []));
    if (chosen.length !== 1) {
      void vscode.window.showInformationMessage('Debug one test at a time: choose a test, not a suite or a project.');
      return;
    }
    const [item] = chosen;
    const dir = projectOf.get(item)!;
    const folder = vscode.workspace.getWorkspaceFolder(vscode.Uri.file(dir));
    await vscode.debug.startDebugging(folder, {
      type: 'tondo',
      request: 'launch',
      name: `Debug ${item.label}`,
      program: dir,
      cwd: dir,
      test: item.id,
    });
  }

  controller.createRunProfile('Run', vscode.TestRunProfileKind.Run, run, true);
  controller.createRunProfile('Debug', vscode.TestRunProfileKind.Debug, (request) => debug(request), true);
  // Projects found at once, so their tests show before the view opens.
  if ((vscode.workspace.workspaceFolders ?? []).some((folder) => isProject(folder.uri.fsPath))) {
    void discoverAll();
  }
}
