// What `tondo` says as JSON, read for the editor: the tests that
// `tondo test --list --test-format json` lists, the events and the report
// of `tondo test --test-format events` (testing 15), the project of a path
// from `tondo graph --json`, and `tondo version --json`.

import * as path from 'node:path';
import { fileURLToPath } from 'node:url';

/** A test or a suite, and where it is. */
export interface Listed {
  id: string;
  parent: string | null;
  name: string;
  suite: boolean;
  file?: string;
  line?: number;
  column?: number;
}

/** How a test ended, as the editor shows it. */
export type Verdict = 'passed' | 'failed' | 'skipped' | 'errored';

export interface Outcome {
  id: string;
  verdict: Verdict;
  /** Why it failed or was skipped, and where. */
  message?: string;
  file?: string;
  line?: number;
  column?: number;
  /** The two sides of the comparison an `assert` failed on. */
  comparison?: { left: string; right: string };
  /** Where the failure happened, the innermost frame first. */
  stack: Frame[];
  /** What it wrote, and what it logged. */
  output: string;
  /** What it logged alone, which comes only with its end. */
  logs: string;
}

export interface Frame {
  function: string;
  file?: string;
  line?: number;
  column?: number;
}

interface Source {
  file?: string;
  line?: number;
  column?: number;
}

interface Entry {
  id: string;
  parent?: string | null;
  name: string;
  source?: Source;
  status?: string;
  decisive_attempt?: number;
  attempts?: Attempt[];
}

interface Attempt {
  failure?: {
    message?: string;
    source?: Source;
    comparison?: { left: string; right: string } | null;
    stack?: { function: string; source?: Source | null; task?: number }[];
  } | null;
  skip?: { reason?: string; source?: Source } | null;
  logs?: string[];
  stdout?: Stream;
  stderr?: Stream;
}

interface Stream {
  encoding: string;
  data: string;
}

/** The suites and tests of a list, suites first. */
export function listed(json: string): Listed[] {
  const list = JSON.parse(json) as { suites?: Entry[]; tests?: Entry[] };
  const node = (entry: Entry, suite: boolean): Listed => ({
    id: entry.id,
    parent: entry.parent ?? null,
    name: entry.name,
    suite,
    file: entry.source?.file,
    line: entry.source?.line,
    column: entry.source?.column,
  });
  return [...(list.suites ?? []).map((s) => node(s, true)), ...(list.tests ?? []).map((t) => node(t, false))];
}

interface Report {
  compiled?: boolean;
  suites?: Entry[];
  tests?: Entry[];
}

/** What the language server sends of a project's tests as they change
 *  (`tondo/tests`). */
export interface Served {
  root: string;
  tests: {
    id: string;
    parent: string | null;
    name: string;
    suite: boolean;
    location?: { uri: string; range: { start: { line: number; character: number } } };
  }[];
}

/** The project and its tests, as a list gives them, with each file whole. */
export function served(params: Served): { dir: string; nodes: Listed[] } {
  const nodes = params.tests.map((test) => ({
    id: test.id,
    parent: test.parent,
    name: test.name,
    suite: test.suite,
    file: test.location ? fileURLToPath(test.location.uri) : undefined,
    line: test.location ? test.location.range.start.line + 1 : undefined,
    column: test.location ? test.location.range.start.character + 1 : undefined,
  }));
  return { dir: fileURLToPath(params.root), nodes };
}

/** How each suite and test of a report ended. */
export function outcomes(json: string): Outcome[] {
  return reported(JSON.parse(json) as Report);
}

function reported(report: Report): Outcome[] {
  return [...(report.suites ?? []), ...(report.tests ?? [])].map((entry) => {
    const attempts = entry.attempts ?? [];
    const attempt = attempts[(entry.decisive_attempt ?? attempts.length) - 1] ?? attempts[attempts.length - 1];
    return outcome(entry.id, entry.status ?? '', attempt);
  });
}

function outcome(id: string, status: string, attempt: Attempt | undefined): Outcome {
  const why = attempt?.failure ?? attempt?.skip ?? undefined;
  const message = attempt?.failure?.message ?? attempt?.skip?.reason;
  const logs = (attempt?.logs ?? []).map((log) => `${log}\n`).join('');
  const stack = (attempt?.failure?.stack ?? [])
    .filter((frame) => !frame.task)
    .map((frame) => ({ function: frame.function, file: frame.source?.file, line: frame.source?.line, column: frame.source?.column }));
  return {
    id,
    verdict: verdict(status),
    message,
    file: why?.source?.file,
    line: why?.source?.line,
    column: why?.source?.column,
    comparison: attempt?.failure?.comparison ?? undefined,
    stack,
    output: text(attempt?.stdout) + text(attempt?.stderr) + logs,
    logs,
  };
}

/** A line of `tondo test --test-format events`: a pass begins with its
 *  tests, a suite or test starts or ends, and the report comes last. */
export type TestEvent =
  | { event: 'pass'; tests: string[] }
  | { event: 'start'; id: string }
  | { event: 'output'; id: string; text: string }
  | { event: 'end'; outcome: Outcome; duration: number }
  | { event: 'report'; compiled: boolean; outcomes: Outcome[] };

export function testEvent(line: string): TestEvent | undefined {
  let value;
  try {
    value = JSON.parse(line) as {
      event?: string;
      id?: string;
      tests?: string[];
      status?: string;
      duration_ms?: number;
      stdout?: Stream;
      stderr?: Stream;
      report?: Report;
    };
  } catch {
    return undefined;
  }
  switch (value.event) {
    case 'pass':
      return { event: 'pass', tests: value.tests ?? [] };
    case 'start':
      return { event: 'start', id: value.id ?? '' };
    case 'output':
      return { event: 'output', id: value.id ?? '', text: text(value.stdout) + text(value.stderr) };
    case 'end':
      return { event: 'end', outcome: outcome(value.id ?? '', value.status ?? '', value as Attempt), duration: value.duration_ms ?? 0 };
    case 'report':
      return { event: 'report', compiled: value.report?.compiled !== false, outcomes: reported(value.report ?? {}) };
    default:
      return undefined;
  }
}

/** What `--report coverage=<path>` writes of a file (testing 15.10): how
 *  many times each line, function and way of a branch ran. */
export interface Covered {
  file: string;
  lines: { line: number; count: number }[];
  functions: { name: string; line: number; count: number }[];
  branches: { branch: { line: number; column: number }; line: number; column: number; label: string; count: number }[];
}

export function covered(json: string): Covered[] {
  const coverage = JSON.parse(json) as { format?: string; files?: Covered[] };
  if (coverage.format !== 'tondo-coverage-0.1/1') {
    throw new Error('not a report of coverage');
  }
  return coverage.files ?? [];
}

/** Where a path is, by `tondo graph --json`: its project, if it is in one,
 *  and the module it belongs to. */
export interface Place {
  root: string;
  project: boolean;
  module: string | null;
}

export function place(json: string): Place {
  const graph = JSON.parse(json) as { format?: string; root: string; project: boolean; focus?: string | null };
  if (!graph.format?.startsWith('tondo-graph-')) {
    throw new Error('tondo graph gave no graph');
  }
  return { root: graph.root, project: graph.project, module: graph.focus ?? null };
}

/** What to build to run or debug a file: its project, or the file. */
export function programOf(where: Place, file: string): { program: string; cwd: string } {
  return where.project ? { program: where.root, cwd: where.root } : { program: file, cwd: path.dirname(file) };
}

/** What the extension needs of `tondo`, as `tondo version --json` lists
 *  its features. */
export const NEEDED = ['json', 'graph', 'test-events', 'test-exact-many', 'test-watch', 'test-coverage', 'lsp-tests'];

/** Why a `tondo` cannot serve this extension, from what `tondo version
 *  --json` printed, or `undefined` when it can. */
export function incompatible(json: string): string | undefined {
  try {
    const version = JSON.parse(json) as { format?: string; version?: string; features?: string[] };
    if (version.format === 'tondo-version-0.1/1' && NEEDED.every((feature) => version.features?.includes(feature))) {
      return undefined;
    }
  } catch {
    // An older tondo prints its version as text.
  }
  const said = json.trim() || 'nothing';
  return `This tondo is older than the extension (it says ${said}): update it with \`sh dist/install.sh\` from Tondo's repository.`;
}

/** The editor's verdict for a status of the report. */
export function verdict(status: string): Verdict {
  switch (status) {
    case 'passed':
    case 'flaky-pass':
      return 'passed';
    case 'skipped':
    case 'blocked-skip':
      return 'skipped';
    case 'failed-error':
    case 'failed-panic':
      return 'failed';
    default:
      return 'errored';
  }
}

function text(stream: Stream | undefined): string {
  if (!stream) {
    return '';
  }
  return stream.encoding === 'base64' ? Buffer.from(stream.data, 'base64').toString('utf8') : stream.data;
}
