// The tests of the workspace's projects in the Testing view: listed by
// `tondo test --list` at first and then by the language server as files
// change, run by `tondo test`, whose events show each result as it comes,
// and debugged by `tondo dap`.

import * as path from 'node:path';
import * as vscode from 'vscode';
import { isProject } from './binary';
import { Ran, exec } from './cli';
import { Listed, Outcome, Served, listed, served, testEvent } from './report';

/** Registers the tests; what it returns takes the language server's. */
export function registerTesting(context: vscode.ExtensionContext, output: vscode.OutputChannel): (params: Served) => void {
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

  /** A project's item, made when it is first seen. */
  function projectItem(dir: string): vscode.TestItem {
    let project = projects.get(dir);
    if (!project) {
      project = controller.createTestItem(`project:${dir}`, path.basename(dir), vscode.Uri.file(dir));
      projects.set(dir, project);
      projectOf.set(project, dir);
      controller.items.add(project);
    }
    return project;
  }

  async function discover(dir: string): Promise<void> {
    const project = projectItem(dir);
    let ran: Ran;
    try {
      ran = await exec(['test', '--list', '--test-format', 'json'], dir);
    } catch (error) {
      project.error = (error as Error).message;
      return;
    }
    try {
      show(dir, listed(ran.stdout));
    } catch {
      project.error = ran.stderr.trim() || 'tondo test --list gave no list';
    }
  }

  /** Shows a project's tests, keeping the items of those that stay where
   *  they were, so that their results stay. */
  function show(dir: string, nodes: Listed[]): void {
    const project = projectItem(dir);
    project.error = undefined;
    const before = new Map<string, vscode.TestItem>();
    const gather = (item: vscode.TestItem): void =>
      item.children.forEach((child) => {
        before.set(child.id, child);
        gather(child);
      });
    gather(project);
    const items = new Map<string, vscode.TestItem>();
    for (const node of nodes) {
      const uri = node.file ? vscode.Uri.file(path.resolve(dir, node.file)) : undefined;
      const kept = before.get(node.id);
      const item = kept && kept.uri?.fsPath === uri?.fsPath ? kept : controller.createTestItem(node.id, node.name, uri);
      item.label = node.name;
      if (node.line) {
        const position = new vscode.Position(node.line - 1, Math.max((node.column ?? 1) - 1, 0));
        item.range = new vscode.Range(position, position);
      }
      projectOf.set(item, dir);
      items.set(node.id, item);
    }
    const children = new Map<vscode.TestItem, vscode.TestItem[]>();
    const roots: vscode.TestItem[] = [];
    for (const node of nodes) {
      const item = items.get(node.id)!;
      const parent = node.parent ? items.get(node.parent) : undefined;
      if (parent) {
        children.set(parent, [...(children.get(parent) ?? []), item]);
      } else {
        roots.push(item);
      }
    }
    items.forEach((item) => item.children.replace(children.get(item) ?? []));
    project.children.replace(roots);
  }

  controller.resolveHandler = async (item) => {
    if (!item) {
      await discoverAll();
    }
  };
  controller.refreshHandler = () => discoverAll();

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
      if (token.isCancellationRequested) {
        break;
      }
      // One run for everything chosen in a project.
      const whole = items.some((item) => item.id.startsWith('project:'));
      const args = ['test', '--test-format', 'events', '--allow-empty'];
      if (!whole) {
        items.forEach((item) => args.push('--exact', item.id));
      }
      const started = items.flatMap((item) => tests(item, []));
      started.forEach((item) => testRun.enqueued(item));
      const known = new Map(started.map((item) => [item.id, item]));
      const locate = (file?: string, line?: number, column?: number) =>
        file && line ? new vscode.Location(vscode.Uri.file(path.join(dir, file)), new vscode.Position(line - 1, Math.max((column ?? 1) - 1, 0))) : undefined;
      const ended = (outcome: Outcome, duration?: number) => {
        const item = known.get(outcome.id);
        if (!item) {
          return;
        }
        const text = outcome.message ?? outcome.verdict;
        // A comparison shows as a diff: what was expected on the right.
        const message = outcome.comparison
          ? vscode.TestMessage.diff(text, outcome.comparison.right, outcome.comparison.left)
          : new vscode.TestMessage(text);
        message.location = locate(outcome.file, outcome.line, outcome.column);
        message.stackTrace = outcome.stack.map((frame) => {
          const at = locate(frame.file, frame.line, frame.column);
          return new vscode.TestMessageStackFrame(frame.function, at?.uri, at?.range.start);
        });
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
      };
      let reported = false;
      const ran: Ran = await exec(args, dir, token, (line) => {
        const event = testEvent(line);
        const item = event && 'id' in event ? known.get(event.id) : undefined;
        switch (event?.event) {
          case 'start':
            if (item) {
              testRun.started(item);
            }
            break;
          case 'output':
            if (item) {
              testRun.appendOutput(event.text.replace(/\r?\n/g, '\r\n'), undefined, item);
            }
            break;
          case 'end': {
            // Each attempt as it ends; a retry may change it.
            ended(event.outcome, event.duration);
            const ending = known.get(event.outcome.id);
            if (ending && event.outcome.logs) {
              testRun.appendOutput(event.outcome.logs.replace(/\r?\n/g, '\r\n'), undefined, ending);
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
        known.forEach((item) => testRun.errored(item, message));
        continue;
      }
      if (ran.stderr.trim()) {
        output.appendLine(ran.stderr.trimEnd());
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
  // Then the language server says what changes as files are edited.
  return (params) => {
    const { dir, nodes } = served(params);
    show(dir, nodes);
  };
}
