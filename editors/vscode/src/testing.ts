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
import { Documents, RUNS_HERE } from './docs';
import { ShowProfile } from './profiles';
import { Covered, Listed, Outcome, Profiled, Served, benchLine, covered, kindMark, listed, searchLine, served, tagLine, testEvent } from './report';

/** Registers the tests; what it returns takes the language server's. */
export function registerTesting(
  context: vscode.ExtensionContext,
  output: vscode.OutputChannel,
  showProfile: ShowProfile,
): (params: Served) => void {
  const controller = vscode.tests.createTestController('tondo', 'Tondo');
  context.subscriptions.push(controller);
  // Each project's item, and the project of each item.
  const projects = new Map<string, vscode.TestItem>();
  const projectOf = new WeakMap<vscode.TestItem, string>();
  // The tests that are benchmarks, which the Benchmark profile measures,
  // and those that fuzz, which the Fuzz profile searches with.
  const benchmark = new vscode.TestTag('bench');
  const fuzzing = new vscode.TestTag('fuzz');
  // Each test's kind, as its item and its line in the editor show it, and
  // the tags its last run said, by id.
  const kinds = new Map<string, Listed>();
  const tagged = new Map<string, string>();
  const shown = new Map<string, string[]>();
  const describe = (item: vscode.TestItem) => {
    const node = kinds.get(item.id);
    const parts = [node ? kindMark(node) : '', tagged.get(item.id) ?? ''].filter((part) => part !== '');
    item.description = parts.length > 0 ? parts.join(' · ') : undefined;
  };
  const marked = vscode.window.createTextEditorDecorationType({
    after: { margin: '0 0 0 1.5em', color: new vscode.ThemeColor('editorCodeLens.foreground') },
  });
  context.subscriptions.push(marked);
  // A benchmark's and a fuzz test's line says so at its end.
  const mark = (editors: readonly vscode.TextEditor[]) => {
    for (const editor of editors) {
      const file = editor.document.uri.fsPath;
      const lines = [...kinds.values()].filter((n) => n.file === file && n.line && kindMark(n) !== '');
      editor.setDecorations(
        marked,
        lines
          .filter((n) => n.line! <= editor.document.lineCount)
          .map((n) => {
            const end = editor.document.lineAt(n.line! - 1).range.end;
            return { range: new vscode.Range(end, end), renderOptions: { after: { contentText: kindMark(n) } } };
          }),
      );
    }
  };
  context.subscriptions.push(vscode.window.onDidChangeVisibleTextEditors(mark));

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
      project.tags = [RUNS_HERE];
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
    for (const id of shown.get(dir) ?? []) {
      kinds.delete(id);
    }
    shown.set(dir, nodes.map((node) => node.id));
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
      // A test of another system's file shows, and does not run here.
      item.tags = node.system ? [] : [RUNS_HERE, ...(node.fuzz ? [fuzzing] : node.bench ? [benchmark] : [])];
      kinds.set(node.id, { ...node, file: node.file ? path.resolve(dir, node.file) : undefined });
      describe(item);
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
    // Which tests fuzz and which measure, for the menus of their items.
    const ids = (kind: 'fuzz' | 'bench') => [...kinds.values()].filter((n) => n[kind] === true).map((n) => n.id);
    void vscode.commands.executeCommand('setContext', 'tondo.fuzzTests', ids('fuzz'));
    void vscode.commands.executeCommand('setContext', 'tondo.benchTests', ids('bench'));
    mark(vscode.window.visibleTextEditors);
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
  function follower(
    dir: string,
    current: () => vscode.TestRun | undefined,
    ended: (compiled: boolean, ids: Set<string>, profiles: Profiled[]) => void,
  ) {
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
      let text = outcome.message ?? outcome.verdict;
      // The input a call of `testing.fuzz` failed with, and where it is.
      const fuzzed = outcome.fuzz.find((call) => call.input);
      if (fuzzed?.input) {
        const reduced = fuzzed.input.reducedFrom ? `, reduced from ${fuzzed.input.reducedFrom} bytes` : '';
        text += `\n\nfuzz \`${fuzzed.name}\` failed with this input${reduced}:\n  ${fuzzed.input.json}`;
        if (fuzzed.input.file) {
          text += `\nsaved in ${fuzzed.input.file}, which every run tries from now on`;
        }
      }
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
      const messages = [message];
      if (fuzzed?.input?.file) {
        const input = new vscode.TestMessage(`the input \`${fuzzed.name}\` failed with`);
        input.location = new vscode.Location(vscode.Uri.file(path.resolve(dir, fuzzed.input.file)), new vscode.Position(0, 0));
        messages.push(input);
      }
      switch (outcome.verdict) {
        case 'passed':
          testRun.passed(test, duration);
          break;
        case 'skipped':
          testRun.skipped(test);
          break;
        case 'failed':
          testRun.failed(test, messages, duration);
          break;
        case 'errored':
          testRun.errored(test, messages, duration);
          break;
      }
    };
    // How each search goes, in its test's output every 5 seconds.
    const searched = new Map<string, number>();
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
        case 'fuzz': {
          const test = item(event.id);
          const key = `${event.id} ${event.name}`;
          if (test && Date.now() - (searched.get(key) ?? 0) >= 5000) {
            searched.set(key, Date.now());
            testRun.appendOutput(`${searchLine(event.name, event.search)}\r\n`, undefined, test);
          }
          break;
        }
        case 'report':
          // The report decides, after every retry.
          event.outcomes.forEach((outcome) => {
            finish(testRun, outcome);
            // What it said of itself with `testing.tags`, beside its name
            // and in its output.
            const test = item(outcome.id);
            const tags = tagLine(outcome.tags);
            if (test) {
              tagged.set(test.id, tags);
              describe(test);
              if (tags) {
                testRun.appendOutput(`tags: ${tags}\r\n`, undefined, test);
              }
            }
          });
          ended(event.compiled, new Set(event.outcomes.map((outcome) => outcome.id)), event.profiles);
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
    const profiling = request.profile?.label === 'Profile';
    const searching = request.profile?.label === 'Fuzz';
    const asked = benchmarking || searching ? [] : (request.include ?? documents.all());
    const docs = asked.filter((item) => documents.owns(item) && !request.exclude?.includes(item));
    if (docs.length > 0) {
      await documents.run(testRun, docs, token);
    }
    for (const [dir, items] of chosen(request)) {
      if (token.isCancellationRequested) {
        break;
      }
      // A benchmark run runs the benchmarks alone, and a fuzz run the
      // tests that fuzz.
      const has = (item: vscode.TestItem, tag: vscode.TestTag) => item.tags.some((t) => t.id === tag.id);
      const all = items.flatMap((item) => tests(item, []));
      // One of another system's file, asked for all the same, says why it
      // does not run.
      for (const item of all) {
        const system = kinds.get(item.id)?.system;
        if (system) {
          testRun.skipped(item);
          testRun.appendOutput(`${item.label} runs on ${system}: this machine does not build its file\r\n`, undefined, item);
        }
      }
      const started = all
        .filter((item) => !kinds.get(item.id)?.system)
        .filter((item) => (!benchmarking || has(item, benchmark)) && (!searching || has(item, fuzzing)));
      started.forEach((item) => testRun.enqueued(item));
      let reported = false;
      let ran = new Set<string>();
      let profiles: Profiled[] = [];
      const follow = follower(dir, () => testRun, (compiled, ids, written) => ((reported = compiled), (ran = ids), (profiles = written)));
      // A profile of each test, or of what the benchmarks measure when
      // only benchmarks are chosen.
      const benchmarks = profiling && started.length > 0 && started.every((item) => item.tags.some((tag) => tag.id === benchmark.id));
      const args = testArgs(dir, items, false, benchmarking || benchmarks);
      if (profiling) {
        args.push('--profile', 'cpu');
      }
      // A search until it is stopped, or for `tondo.fuzz.time` seconds.
      if (searching) {
        const time = vscode.workspace.getConfiguration('tondo', vscode.Uri.file(dir)).get<number>('fuzz.time', 0);
        args.push('--fuzz', ...(time > 0 ? ['--fuzz-time', String(time)] : []));
      }
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
      if (profiling && profiles.length === 0) {
        void vscode.window.showInformationMessage(
          'Tondo: no profile, as no test ran a millisecond of processor, the least a sample takes. A benchmark (testing.bench) runs fast code often enough to profile it.',
        );
      } else if (profiling) {
        const picks = profiles.map((profiled) => ({
          label: profiled.test.split('::').pop() ?? profiled.test,
          description: profiled.bench,
          detail: profiled.file,
          file: profiled.file,
        }));
        const chosen = picks.length === 1 ? picks[0] : await vscode.window.showQuickPick(picks, { placeHolder: "Which test's profile?" });
        if (chosen) {
          await showProfile(dir, path.resolve(dir, chosen.file));
        }
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

  controller.createRunProfile('Run', vscode.TestRunProfileKind.Run, run, true, RUNS_HERE, true);
  const measure = controller.createRunProfile('Benchmark', vscode.TestRunProfileKind.Run, run, false, benchmark);
  const fuzz = controller.createRunProfile('Fuzz', vscode.TestRunProfileKind.Run, run, false, fuzzing);
  const profile = controller.createRunProfile('Profile', vscode.TestRunProfileKind.Run, run, false, RUNS_HERE);
  // The profile's run, beside each test's Run and Debug, and in their
  // menus, where VS Code offers only the default profiles.
  const included = (test?: vscode.TestItem, chosen?: vscode.TestItem[]) => (chosen?.length ? chosen : test ? [test] : undefined);
  context.subscriptions.push(
    vscode.commands.registerCommand('tondo.profileTest', (test?: vscode.TestItem, chosen?: vscode.TestItem[]) => {
      const request = new vscode.TestRunRequest(included(test, chosen), undefined, profile);
      return run(request, new vscode.CancellationTokenSource().token);
    }),
    vscode.commands.registerCommand('tondo.benchmarkTest', (test?: vscode.TestItem, chosen?: vscode.TestItem[]) => {
      const request = new vscode.TestRunRequest(included(test, chosen), undefined, measure);
      return run(request, new vscode.CancellationTokenSource().token);
    }),
    // A search runs until it is stopped, from a notification.
    vscode.commands.registerCommand('tondo.fuzzTest', (test?: vscode.TestItem, chosen?: vscode.TestItem[]) => {
      const request = new vscode.TestRunRequest(included(test, chosen), undefined, fuzz);
      const title = `Fuzzing ${test?.label ?? 'the tests that fuzz'}: Stop ends the search and keeps what it found`;
      return vscode.window.withProgress({ location: vscode.ProgressLocation.Notification, title, cancellable: true }, (_, token) =>
        run(request, token),
      );
    }),
  );
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
  controller.createRunProfile('Debug', vscode.TestRunProfileKind.Debug, (request) => debug(request), true, RUNS_HERE);
  const coverage = controller.createRunProfile('Coverage', vscode.TestRunProfileKind.Coverage, run, true, RUNS_HERE);
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
