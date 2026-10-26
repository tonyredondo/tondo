// The tests of the workspace's projects in the Testing view: listed by
// `tondo test --list` at first and then by the language server as files
// change, run by `tondo test`, whose events show each result as it comes,
// and debugged by `tondo dap`.

import * as fs from 'node:fs/promises';
import * as os from 'node:os';
import * as path from 'node:path';
import * as vscode from 'vscode';
import { isProject } from './binary';
import { Ran, exec, settings } from './cli';
import { Documents } from './docs';
import { Covered, Listed, Outcome, Served, benchLine, covered, listed, served, testEvent } from './report';

/** Registers the tests; what it returns takes the language server's. */
export function registerTesting(context: vscode.ExtensionContext, output: vscode.OutputChannel): (params: Served) => void {
  const controller = vscode.tests.createTestController('tondo', 'Tondo');
  context.subscriptions.push(controller);
  // Each project's item, and the project of each item.
  const projects = new Map<string, vscode.TestItem>();
  const projectOf = new WeakMap<vscode.TestItem, string>();
  // The tests that are benchmarks, which the Benchmark profile measures.
  const benchmark = new vscode.TestTag('bench');

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
      item.tags = node.bench ? [benchmark] : [];
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
      await Promise.all([discoverAll(), documents.discover()]);
    }
  };
  controller.refreshHandler = () => Promise.all([discoverAll(), documents.discover()]).then(() => undefined);
  // The examples of Markdown documents, which `tondo doc-test` checks.
  const documents = new Documents(controller);
  context.subscriptions.push(
    vscode.workspace.onDidSaveTextDocument((document) => {
      if (document.languageId === 'markdown') {
        void documents.read(document.uri);
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

  /** The items asked for, by project: everything, or those chosen. */
  function chosen(request: vscode.TestRunRequest): Map<string, vscode.TestItem[]> {
    const included = request.include ?? [...projects.values()];
    const excluded = new Set(request.exclude ?? []);
    const byProject = new Map<string, vscode.TestItem[]>();
    for (const item of included) {
      const dir = projectOf.get(item);
      if (dir && !excluded.has(item)) {
        byProject.set(dir, [...(byProject.get(dir) ?? []), item]);
      }
    }
    return byProject;
  }

  /** `tondo test` with the items of a project, as events; measuring
   *  benchmarks with `--bench` and what the settings add. */
  function testArgs(dir: string, items: vscode.TestItem[], watch: boolean, bench = false): string[] {
    const args = ['test', '--test-format', 'events', '--allow-empty'];
    if (watch) {
      args.push('--watch');
    }
    if (bench) {
      args.push('--bench', ...vscode.workspace.getConfiguration('tondo', vscode.Uri.file(dir)).get<string[]>('bench.args', []));
    }
    if (!items.some((item) => item.id.startsWith('project:'))) {
      items.forEach((item) => args.push('--exact', item.id));
    }
    // What the settings add, as `--jobs 4`.
    args.push(...settings('test', dir).args);
    return args;
  }

  /** Shows the events of `tondo test` in a project on the run `current`
   *  gives; `ended` is told when a report ends a run, and whether the
   *  tests compiled. */
  function follower(dir: string, current: () => vscode.TestRun | undefined, ended: (compiled: boolean, ids: Set<string>) => void) {
    const item = (id: string) => {
      const found = projects.get(dir);
      let match: vscode.TestItem | undefined;
      const find = (parent: vscode.TestItem) =>
        parent.children.forEach((child) => {
          if (child.id === id) {
            match = child;
          } else if (!match) {
            find(child);
          }
        });
      if (found) {
        find(found);
      }
      return match;
    };
    const locate = (file?: string, line?: number, column?: number) =>
      file && line ? new vscode.Location(vscode.Uri.file(path.resolve(dir, file)), new vscode.Position(line - 1, Math.max((column ?? 1) - 1, 0))) : undefined;
    const finish = (testRun: vscode.TestRun, outcome: Outcome, duration?: number) => {
      const test = item(outcome.id);
      if (!test) {
        return;
      }
      const text = outcome.message ?? outcome.verdict;
      // A comparison shows as a diff: what was expected on the right.
      const message = outcome.comparison
        ? vscode.TestMessage.diff(text, outcome.comparison.right, outcome.comparison.left)
        : new vscode.TestMessage(text);
      message.location = locate(outcome.file, outcome.line, outcome.column);
      // A snapshot that differs offers to be written as it is now.
      if (outcome.code === 'P2007') {
        message.contextValue = 'tondo.snapshot';
      }
      message.stackTrace = outcome.stack.map((frame) => {
        const at = locate(frame.file, frame.line, frame.column);
        return new vscode.TestMessageStackFrame(frame.function, at?.uri, at?.range.start);
      });
      switch (outcome.verdict) {
        case 'passed':
          testRun.passed(test, duration);
          break;
        case 'skipped':
          testRun.skipped(test);
          break;
        case 'failed':
          testRun.failed(test, message, duration);
          break;
        case 'errored':
          testRun.errored(test, message, duration);
          break;
      }
    };
    return (line: string) => {
      const event = testEvent(line);
      const testRun = current();
      if (!event || !testRun) {
        return;
      }
      switch (event.event) {
        case 'pass':
          event.tests.forEach((id) => {
            const test = item(id);
            if (test) {
              testRun.enqueued(test);
            }
          });
          break;
        case 'start': {
          const test = item(event.id);
          if (test) {
            testRun.started(test);
          }
          break;
        }
        case 'output': {
          const test = item(event.id);
          if (test) {
            testRun.appendOutput(event.text.replace(/\r?\n/g, '\r\n'), undefined, test);
          }
          break;
        }
        case 'end': {
          // Each attempt as it ends; a retry may change it.
          finish(testRun, event.outcome, event.duration);
          const test = item(event.outcome.id);
          if (test && event.outcome.logs) {
            testRun.appendOutput(event.outcome.logs.replace(/\r?\n/g, '\r\n'), undefined, test);
          }
          // What its benchmarks measured.
          if (test && event.outcome.benchmarks.length > 0) {
            const lines = event.outcome.benchmarks.map((b) => `${test.label} ${benchLine(b)}\r\n`).join('');
            testRun.appendOutput(lines, undefined, test);
          }
          break;
        }
        case 'report':
          // The report decides, after every retry.
          event.outcomes.forEach((outcome) => finish(testRun, outcome));
          ended(event.compiled, new Set(event.outcomes.map((outcome) => outcome.id)));
          break;
      }
    };
  }

  // What each file's coverage shows when it is opened.
  const details = new WeakMap<vscode.FileCoverage, vscode.FileCoverageDetail[]>();

  /** Shows a project's coverage in the run. */
  function cover(testRun: vscode.TestRun, dir: string, files: Covered[]): void {
    for (const file of files) {
      const statements = new Map<number, vscode.StatementCoverage>();
      for (const { line, count } of file.lines) {
        statements.set(line, new vscode.StatementCoverage(count, new vscode.Position(line - 1, 0), []));
      }
      // Each way of a branch belongs to the line of its branch.
      for (const way of file.branches) {
        const at = way.branch.line;
        if (!statements.has(at)) {
          statements.set(at, new vscode.StatementCoverage(0, new vscode.Position(at - 1, 0), []));
        }
        const position = new vscode.Position(way.line - 1, Math.max(way.column - 1, 0));
        statements.get(at)!.branches.push(new vscode.BranchCoverage(way.count, position, way.label));
      }
      const declarations = file.functions.map(
        (f) => new vscode.DeclarationCoverage(f.name, f.count, new vscode.Position(f.line - 1, 0)),
      );
      const shown = [...statements.values(), ...declarations];
      const coverage = vscode.FileCoverage.fromDetails(vscode.Uri.file(path.resolve(dir, file.file)), shown);
      details.set(coverage, shown);
      testRun.addCoverage(coverage);
    }
  }

  async function run(request: vscode.TestRunRequest, token: vscode.CancellationToken): Promise<void> {
    if (request.continuous) {
      await Promise.all([...chosen(request)].map(([dir, items]) => watch(request, dir, items, token)));
      return;
    }
    const testRun = controller.createTestRun(request);
    const benchmarking = request.profile?.label === 'Benchmark';
    const asked = benchmarking ? [] : (request.include ?? documents.all());
    const docs = asked.filter((item) => documents.owns(item) && !request.exclude?.includes(item));
    if (docs.length > 0) {
      await documents.run(testRun, docs, token);
    }
    for (const [dir, items] of chosen(request)) {
      if (token.isCancellationRequested) {
        break;
      }
      // A benchmark run runs the benchmarks alone.
      const started = items
        .flatMap((item) => tests(item, []))
        .filter((item) => !benchmarking || item.tags.some((tag) => tag.id === benchmark.id));
      started.forEach((item) => testRun.enqueued(item));
      let reported = false;
      let ran = new Set<string>();
      const follow = follower(dir, () => testRun, (compiled, ids) => ((reported = compiled), (ran = ids)));
      const args = testArgs(dir, items, false, benchmarking);
      const report = path.join(os.tmpdir(), `tondo-coverage-${process.pid}-${Date.now()}.json`);
      const coverage = request.profile?.kind === vscode.TestRunProfileKind.Coverage;
      if (coverage) {
        args.push('--report', `coverage=${report}`);
      }
      // What the benchmarks measured, as a page beside the code.
      const measured = path.join(os.tmpdir(), `tondo-benchmarks-${path.basename(dir)}.md`);
      if (benchmarking) {
        args.push('--report', `markdown=${measured}`);
      }
      const done: Ran = await exec(args, dir, token, follow, settings('test', dir).env).catch((error: Error) => ({
        code: null,
        stdout: '',
        stderr: error.message,
      }));
      if (!reported) {
        // It did not compile, or tondo failed: why goes on every test.
        const message = new vscode.TestMessage(done.stderr.trim() || 'tondo test gave no report');
        started.forEach((item) => testRun.errored(item, message));
        continue;
      }
      if (done.stderr.trim()) {
        output.appendLine(done.stderr.trimEnd());
      }
      if (coverage) {
        try {
          cover(testRun, dir, covered(await fs.readFile(report, 'utf8')));
        } catch (error) {
          output.appendLine(`no coverage: ${(error as Error).message}`);
        }
        await fs.rm(report, { force: true });
      }
      if (benchmarking) {
        void vscode.commands.executeCommand('markdown.showPreviewToSide', vscode.Uri.file(measured));
      }
      // What the report does not name did not run.
      started.filter((item) => !ran.has(item.id)).forEach((item) => testRun.skipped(item));
    }
    testRun.end();
  }

  /** A continuous run: `tondo test --watch`, with a run of the view for
   *  each run of the tests, until it is cancelled. */
  async function watch(request: vscode.TestRunRequest, dir: string, items: vscode.TestItem[], token: vscode.CancellationToken): Promise<void> {
    let testRun: vscode.TestRun | undefined;
    const follow = follower(dir, () => {
      testRun ??= controller.createTestRun(request, undefined, true);
      return testRun;
    }, (compiled) => {
      if (!compiled) {
        const message = new vscode.TestMessage('the tests do not compile: Problems shows why');
        items.flatMap((item) => tests(item, [])).forEach((item) => testRun?.errored(item, message));
      }
      testRun?.end();
      testRun = undefined;
    });
    const done = await exec(testArgs(dir, items, true), dir, token, follow, settings('test', dir).env).catch((error: Error) => ({
      code: null,
      stdout: '',
      stderr: error.message,
    }));
    testRun?.end();
    if (!token.isCancellationRequested && done.stderr.trim()) {
      output.appendLine(done.stderr.trimEnd());
    }
  }

  async function debug(request: vscode.TestRunRequest): Promise<void> {
    const chosen = (request.include ?? []).flatMap((item) => tests(item, []));
    if (chosen.length !== 1) {
      void vscode.window.showInformationMessage('Debug one test at a time: choose a test, not a suite or a project.');
      return;
    }
    const [item] = chosen;
    const dir = projectOf.get(item);
    if (!dir) {
      void vscode.window.showInformationMessage("A document's examples run, but are not debugged: Run them instead.");
      return;
    }
    const folder = vscode.workspace.getWorkspaceFolder(vscode.Uri.file(dir));
    await vscode.debug.startDebugging(folder, {
      type: 'tondo',
      request: 'launch',
      name: `Debug ${item.label}`,
      program: dir,
      cwd: dir,
      test: item.id,
      env: settings('test', dir).env,
    });
  }

  controller.createRunProfile('Run', vscode.TestRunProfileKind.Run, run, true, undefined, true);
  controller.createRunProfile('Benchmark', vscode.TestRunProfileKind.Run, run, false, benchmark);
  // A snapshot written as the test now makes it, then the test again.
  context.subscriptions.push(
    vscode.commands.registerCommand('tondo.updateSnapshot', async (asked: { test: vscode.TestItem }) => {
      const dir = projectOf.get(asked.test);
      if (!dir) {
        return;
      }
      const ran = await exec(['test', '--update-snapshots', '--exact', asked.test.id], dir);
      if (ran.code !== 0) {
        void vscode.window.showErrorMessage(ran.stderr.trim() || ran.stdout.trim() || 'tondo test failed');
        return;
      }
      await run(new vscode.TestRunRequest([asked.test]), new vscode.CancellationTokenSource().token);
    }),
  );
  controller.createRunProfile('Debug', vscode.TestRunProfileKind.Debug, (request) => debug(request), true);
  const coverage = controller.createRunProfile('Coverage', vscode.TestRunProfileKind.Coverage, run, true);
  coverage.loadDetailedCoverage = async (_, file) => details.get(file) ?? [];
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
