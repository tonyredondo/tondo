// What `tondo` says, read for the editor: the tests that `tondo test
// --list --test-format json` lists, the events and the report of `tondo
// test --test-format events` (testing 15), the project of a path from
// `tondo graph --json`, the diagnostics of `tondo check --json`, what
// `tondo doc-test` says of a document's examples, and `tondo version
// --json`.

import * as path from 'node:path';
import { fileURLToPath } from 'node:url';

/** A test or a suite, and where it is. */
export interface Listed {
  id: string;
  parent: string | null;
  name: string;
  suite: boolean;
  /** A test whose body calls `testing.bench` (testing 10.4). */
  bench?: boolean;
  file?: string;
  line?: number;
  column?: number;
}

/** What a benchmark measured, and how it compares with its baseline. */
export interface Measured {
  name: string;
  /** Nanoseconds a run takes, the median of the samples, and their spread. */
  median: number;
  deviation: number;
  allocations: number;
  bytes: number;
  change?: { percent: number; verdict: string; regressed?: string };
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
  /** The code of the panic or error, as `P2007` for a snapshot that
   *  differs. */
  code?: string;
  /** Where the failure happened, the innermost frame first. */
  stack: Frame[];
  /** What it wrote, and what it logged. */
  output: string;
  /** What it logged alone, which comes only with its end. */
  logs: string;
  /** What its benchmarks measured, under `--bench`. */
  benchmarks: Measured[];
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
  bench?: boolean;
}

interface Benchmark {
  name: string;
  ns_per_op: { median: number; deviation: number };
  allocations_per_op: number;
  bytes_per_op: number;
  baseline?: { change_percent: number; verdict: string; regressed: string | null } | null;
}

interface Attempt {
  failure?: {
    message?: string;
    source?: Source;
    comparison?: { left: string; right: string } | null;
    code?: string | null;
    stack?: { function: string; source?: Source | null; task?: number }[];
  } | null;
  skip?: { reason?: string; source?: Source } | null;
  logs?: string[];
  stdout?: Stream;
  stderr?: Stream;
  benchmarks?: Benchmark[];
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
    bench: entry.bench === true,
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
  profiles?: { test: string; bench: string | null; profile: string }[] | null;
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
    bench?: boolean;
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
    bench: test.bench === true,
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
    code: attempt?.failure?.code ?? undefined,
    stack,
    output: text(attempt?.stdout) + text(attempt?.stderr) + logs,
    logs,
    benchmarks: (attempt?.benchmarks ?? []).map((b) => ({
      name: b.name,
      median: b.ns_per_op.median,
      deviation: b.ns_per_op.deviation,
      allocations: b.allocations_per_op,
      bytes: b.bytes_per_op,
      change: b.baseline
        ? { percent: b.baseline.change_percent, verdict: b.baseline.verdict, regressed: b.baseline.regressed ?? undefined }
        : undefined,
    })),
  };
}

/** A time per run in the unit that reads best, as `tondo test` shows it. */
export function duration(nanoseconds: number): string {
  const [value, unit] =
    nanoseconds < 1e3 ? [nanoseconds, 'ns'] : nanoseconds < 1e6 ? [nanoseconds / 1e3, 'µs'] : nanoseconds < 1e9 ? [nanoseconds / 1e6, 'ms'] : [nanoseconds / 1e9, 's'];
  return `${value.toFixed(value < 10 ? 3 : value < 100 ? 2 : 1)} ${unit}`;
}

/** A benchmark's line: its time per run and spread, its allocations, and
 *  how it changed. */
export function benchLine(measured: Measured): string {
  const spread = measured.median > 0 ? (measured.deviation / measured.median) * 100 : 0;
  const count = (n: number) => (Number.isInteger(n) ? `${n}` : n.toFixed(2));
  let line = `${measured.name}  ${duration(measured.median)}/op  ±${spread.toFixed(1)}%  ${count(measured.allocations)} allocs/op  ${count(measured.bytes)} B/op`;
  if (measured.change) {
    const sign = measured.change.percent >= 0 ? '+' : '';
    line += `  ${sign}${measured.change.percent.toFixed(1)}% ${measured.change.verdict}`;
    if (measured.change.regressed) {
      line += `, regressed: ${measured.change.regressed}`;
    }
  }
  return line;
}

/** A line of `tondo test --test-format events`: a pass begins with its
 *  tests, a suite or test starts or ends, and the report comes last. */
export type TestEvent =
  | { event: 'pass'; tests: string[] }
  | { event: 'start'; id: string }
  | { event: 'output'; id: string; text: string }
  | { event: 'end'; outcome: Outcome; duration: number }
  | { event: 'report'; compiled: boolean; outcomes: Outcome[]; profiles: string[] };

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
      return {
        event: 'report',
        compiled: value.report?.compiled !== false,
        outcomes: reported(value.report ?? {}),
        profiles: (value.report?.profiles ?? []).map((written) => written.profile),
      };
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
export const NEEDED = ['json', 'graph', 'test-events', 'test-exact-many', 'test-watch', 'test-coverage', 'lsp-tests', 'explain', 'new', 'test-bench', 'profile'];

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

/** A program `tondo ps --json` lists: one that started with
 *  `TONDO_DIAGNOSTICS=1`, or a debug build. */
export interface Running {
  pid: number;
  program: string;
  arguments: string[];
  uptime_ms: number;
}

export function running(json: string): Running[] {
  const listed = JSON.parse(json) as { format?: string; programs?: Running[] };
  if (listed.format !== 'tondo-ps-0.1/1') {
    throw new Error('tondo ps gave no list');
  }
  return listed.programs ?? [];
}

/** A line of `tondo counters --json`: each counter, as a row to show. */
export function counterRows(line: string): { name: string; value: string }[] | undefined {
  let sample: { counters?: { name: string; unit?: string; value: number; rate?: number }[] };
  try {
    sample = JSON.parse(line) as typeof sample;
  } catch {
    return undefined;
  }
  return (sample.counters ?? []).map((counter) => {
    const unit = counter.unit === 'us' ? ' µs' : counter.unit ? ` ${counter.unit}` : '';
    const rate = counter.rate === undefined ? '' : `, ${counter.rate}${unit}/s`;
    return { name: counter.name, value: `${counter.value}${unit}${rate}` };
  });
}

/** What `tondo trace --json` wrote: the timeline, for Perfetto, and the
 *  CPU profile. */
export function traced(json: string): { timeline: string; profile: string } {
  const trace = JSON.parse(json) as { format?: string; timeline?: string; profile?: string; message?: string };
  if (trace.format !== 'tondo-trace-0.1/1' || !trace.timeline || !trace.profile) {
    throw new Error(trace.message ?? 'tondo trace gave no trace');
  }
  return { timeline: trace.timeline, profile: trace.profile };
}

/** A project's packages, each with its modules, each with its files and
 *  imports, from `tondo graph --json`; the standard modules last, as the
 *  package `std`. */
export interface Graphed {
  name: string;
  dir?: string;
  modules: { name: string; files: string[]; imports: { module: string; file: string; line: number }[] }[];
}

export function graphed(json: string): Graphed[] {
  const graph = JSON.parse(json) as {
    format?: string;
    packages?: { name: string; dir: string }[];
    modules?: { name: string; package: string; files: string[]; imports: { module: string; file: string; line: number }[] }[];
  };
  if (graph.format !== 'tondo-graph-0.1/1') {
    throw new Error('tondo graph gave no graph');
  }
  const packages: Graphed[] = (graph.packages ?? []).map((p) => ({ name: p.name, dir: p.dir, modules: [] }));
  for (const module of graph.modules ?? []) {
    let found = packages.find((p) => p.name === module.package);
    if (!found) {
      found = { name: module.package, modules: [] };
      packages.push(found);
    }
    found.modules.push({ name: module.name, files: module.files, imports: module.imports });
  }
  return packages;
}

/** A diagnostic of `tondo check --json` or `tondo build --json`, its
 *  lines and columns counted from 0, its file from the project's
 *  directory, or the file's. */
export interface Problem {
  file?: string;
  severity: 'error' | 'warning';
  code: string;
  message: string;
  start: { line: number; column: number };
  end: { line: number; column: number };
}

/** Whether `tondo check` or `tondo build` went well, its diagnostics, and
 *  what it built. */
export function checked(json: string): { ok: boolean; output?: string; problems: Problem[] } {
  const report = JSON.parse(json) as {
    format?: string;
    ok: boolean;
    output?: string;
    message?: string;
    diagnostics?: {
      file: string | null;
      severity: 'error' | 'warning';
      code: string;
      message: string;
      range: { start: { line: number; column: number }; end: { line: number; column: number } } | null;
    }[];
  };
  if (!report.format?.startsWith('tondo-')) {
    throw new Error('tondo gave no report');
  }
  const at = (point?: { line: number; column: number }) => ({ line: point?.line ?? 0, column: point?.column ?? 0 });
  const problems = (report.diagnostics ?? []).map((d) => ({
    file: d.file ?? undefined,
    severity: d.severity,
    code: d.code,
    message: d.message,
    start: at(d.range?.start),
    end: at(d.range?.end),
  }));
  // An error that is no diagnostic, as a manifest that does not read.
  if (!report.ok && problems.length === 0 && report.message) {
    problems.push({ file: undefined, severity: 'error', code: '', message: report.message, start: at(), end: at() });
  }
  return { ok: report.ok, output: report.output, problems };
}

/** A diagnostic as the terminal shows it, where the Problems panel's
 *  links find it. */
export function rendered(problem: Problem): string {
  const head = `${problem.severity}${problem.code ? `[${problem.code}]` : ''}: ${problem.message}`;
  return problem.file ? `${head}\n  --> ${problem.file}:${problem.start.line + 1}:${problem.start.column + 1}` : head;
}

/** Where each example of a Markdown document starts, by line from 0, and
 *  how it is checked: `tondo`, `tondo compile-fail E1102`… */
export function examples(text: string): { line: number; header: string }[] {
  const found: { line: number; header: string }[] = [];
  text.split('\n').forEach((line, index) => {
    const header = line.replace(/\r$/, '');
    if (header.startsWith('~~~tondo')) {
      found.push({ line: index, header: header.slice(3) });
    }
  });
  return found;
}

/** What `tondo doc-test` said of each example that failed, by the line of
 *  its fence, counted from 1. */
export function failures(stderr: string): Map<number, string> {
  const found = new Map<number, string>();
  let current: number | undefined;
  for (const line of stderr.split('\n')) {
    const start = /^.*?\.md:(\d+): (.*)$/.exec(line);
    if (start) {
      current = Number(start[1]);
      found.set(current, start[2]);
    } else if (current !== undefined && line.trim()) {
      found.set(current, `${found.get(current)}\n${line}`);
    }
  }
  return found;
}

/** What `tondo profile show --json` says of a profile's lines: their
 *  file, line and what they took, their own and with what they call. */
export interface HotLines {
  type: string;
  unit: string;
  total: number;
  lines: { file: string; line: number; flat: number; cum: number }[];
}

export function hotLines(json: string): HotLines {
  const shown = JSON.parse(json) as { format?: string; type: string; unit: string; total: number; lines: HotLines['lines'] };
  if (shown.format !== 'tondo-profile-show-0.1/1') {
    throw new Error('tondo profile show gave no profile');
  }
  return { type: shown.type, unit: shown.unit, total: shown.total, lines: shown.lines };
}

/** What a line took, as its end shows it: its share and amount, and with
 *  what it calls when that is more; nothing for a line that took little. */
export function share(flat: number, cum: number, total: number, unit: string): string | undefined {
  if (total <= 0 || Math.max(flat, cum) / total < 0.005) {
    return undefined;
  }
  const percent = (value: number) => `${((value / total) * 100).toFixed(1)}%`;
  const amount = (value: number) =>
    unit === 'nanoseconds'
      ? value >= 1e9
        ? `${(value / 1e9).toFixed(2)}s`
        : `${Math.round(value / 1e6)}ms`
      : unit === 'bytes'
        ? value >= 1 << 20
          ? `${(value / (1 << 20)).toFixed(1)}MiB`
          : `${(value / 1024).toFixed(1)}KiB`
        : `${value}`;
  const own = flat > 0 ? `${percent(flat)} (${amount(flat)})` : '';
  const calls = cum > flat ? `${percent(cum)} with what it calls` : '';
  return `🔥 ${[own, calls].filter((part) => part).join(', ')}`;
}
