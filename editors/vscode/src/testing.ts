// The tests of the workspace's projects in the Testing view: listed by
// `tondo test --list`, run by `tondo test`, debugged by `tondo dap`.

import { spawn } from 'node:child_process';
import * as path from 'node:path';
import * as vscode from 'vscode';
import { isProject } from './binary';
import { tondo } from './extension';
import { listed, outcomes } from './report';

/** What `tondo` printed, and how it ended. */
interface Ran {
  code: number | null;
  stdout: string;
  stderr: string;
}

function exec(args: string[], cwd: string, token?: vscode.CancellationToken): Promise<Ran> {
  return new Promise((resolve, reject) => {
    const child = spawn(tondo(), args, { cwd });
    let stdout = '';
    let stderr = '';
    child.stdout.on('data', (data: Buffer) => (stdout += data.toString()));
    child.stderr.on('data', (data: Buffer) => (stderr += data.toString()));
    child.on('error', reject);
    child.on('close', (code) => resolve({ code, stdout, stderr }));
    token?.onCancellationRequested(() => child.kill());
  });
}

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

  // A test file saved lists its project's tests again.
  const pending = new Map<string, NodeJS.Timeout>();
  context.subscriptions.push(
    vscode.workspace.onDidSaveTextDocument((document) => {
      if (document.languageId !== 'tondo') {
        return;
      }
      const dir = [...projects.keys()].find((d) => document.uri.fsPath.startsWith(d + path.sep));
      if (dir) {
        clearTimeout(pending.get(dir));
        pending.set(dir, setTimeout(() => void discover(dir), 500));
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
      started.forEach((item) => testRun.started(item));
      const known = new Map(started.map((item) => [item.id, item]));
      for (const selection of selections) {
        if (token.isCancellationRequested) {
          break;
        }
        const args = ['test', '--test-format', 'json', '--allow-empty'];
        if (selection) {
          args.push('--exact', selection);
        }
        const ran = await exec(args, dir, token).catch((error: Error) => ({ code: null, stdout: '', stderr: error.message }));
        let ended;
        try {
          ended = outcomes(ran.stdout);
        } catch {
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
        for (const outcome of ended) {
          const item = known.get(outcome.id);
          if (!item) {
            continue;
          }
          known.delete(outcome.id);
          const location =
            outcome.file && outcome.line
              ? new vscode.Location(
                  vscode.Uri.file(path.join(dir, outcome.file)),
                  new vscode.Position(outcome.line - 1, Math.max((outcome.column ?? 1) - 1, 0)),
                )
              : undefined;
          if (outcome.output) {
            testRun.appendOutput(outcome.output.replace(/\r?\n/g, '\r\n'), location, item);
          }
          const message = new vscode.TestMessage(outcome.message ?? outcome.verdict);
          message.location = location;
          switch (outcome.verdict) {
            case 'passed':
              testRun.passed(item);
              break;
            case 'skipped':
              testRun.skipped(item);
              break;
            case 'failed':
              testRun.failed(item, message);
              break;
            case 'errored':
              testRun.errored(item, message);
              break;
          }
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
