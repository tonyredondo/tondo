// What `tondo` says, read for the editor: the tests that `tondo test
// --list --test-format json` lists, the events and the report of `tondo
// test --test-format events` (testing 15), the project of a path from
// `tondo graph --json`, the diagnostics of `tondo check --json`, what
// `tondo doc-test` says of a document's examples, what a reload of a live
// build did, `tondo version --json`, and the debugger's tables and tasks.

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
  /** A test whose body calls `testing.fuzz` (testing 10.6). */
  fuzz?: boolean;
  file?: string;
  line?: number;
  column?: number;
  /** For a test of another system's file, that system, which this
   *  machine does not run (language 6.8). */
  system?: string;
}

/** What a call of `testing.fuzz` did: the inputs it ran, the one it
 *  failed with and where it is kept, and under `--fuzz` its search. */
export interface Fuzzed {
  name: string;
  inputs: number;
  input?: { json: string; file?: string; seed?: number; reducedFrom?: number };
  search?: Searched;
}

/** How a search goes: its inputs, time, corpus and coverage. */
export interface Searched {
  inputs: number;
  elapsedMs: number;
  corpus: number;
  reached: number;
  total: number;
  newMs?: number;
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
  /** What looks wrong without failing it, as an update tasks can lose. */
  warnings: string[];
  /** What its benchmarks measured, under `--bench`. */
  benchmarks: Measured[];
  /** What its calls of `testing.fuzz` did (testing 15.15). */
  fuzz: Fuzzed[];
  /** What it said of itself with `testing.tags`, which only the report
   *  has (testing 9.4). */
  tags: Record<string, string>;
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
  fuzz?: boolean;
}

interface FuzzJson {
  name: string;
  inputs?: number;
  input?: { json: string; file?: string | null; seed?: number | null; reduced_from?: number | null } | null;
  search?: SearchJson | null;
}

interface SearchJson {
  inputs?: number;
  elapsed_ms?: number;
  corpus?: number;
  coverage?: { reached?: number; total?: number };
  new_ms?: number;
}

function searched(search: SearchJson, inputs: number): Searched {
  return {
    inputs: search.inputs ?? inputs,
    elapsedMs: search.elapsed_ms ?? 0,
    corpus: search.corpus ?? 0,
    reached: search.coverage?.reached ?? 0,
    total: search.coverage?.total ?? 0,
    newMs: search.new_ms,
  };
}

function fuzzed(call: FuzzJson): Fuzzed {
  const input = call.input
    ? {
        json: call.input.json,
        file: call.input.file ?? undefined,
        seed: call.input.seed ?? undefined,
        reducedFrom: call.input.reduced_from ?? undefined,
      }
    : undefined;
  const inputs = call.inputs ?? 0;
  return { name: call.name, inputs, input, search: call.search ? searched(call.search, inputs) : undefined };
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
  warnings?: string[];
  stdout?: Stream;
  stderr?: Stream;
  benchmarks?: Benchmark[];
  tags?: Record<string, string>;
  fuzz?: FuzzJson[];
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
    fuzz: entry.fuzz === true,
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
    fuzz?: boolean;
    location?: { uri: string; range: { start: { line: number; character: number } } };
    system?: string;
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
    fuzz: test.fuzz === true,
    file: test.location ? fileURLToPath(test.location.uri) : undefined,
    line: test.location ? test.location.range.start.line + 1 : undefined,
    column: test.location ? test.location.range.start.character + 1 : undefined,
    ...(test.system ? { system: test.system } : {}),
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
    warnings: attempt?.warnings ?? [],
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
    tags: attempt?.tags ?? {},
    fuzz: (attempt?.fuzz ?? []).map(fuzzed),
  };
}

/** A test's tags in a line: `component=users, kind=integration`. */
export function tagLine(tags: Record<string, string>): string {
  return Object.entries(tags)
    .map(([key, value]) => `${key}=${value}`)
    .join(', ');
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
  | { event: 'fuzz'; id: string; name: string; search: Searched }
  | { event: 'report'; compiled: boolean; outcomes: Outcome[]; profiles: Profiled[] };

/** A profile `--profile` wrote: of a test, or of one of its benchmarks. */
export interface Profiled {
  test: string;
  bench?: string;
  file: string;
}

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
    case 'fuzz': {
      const search = value as SearchJson & { name?: string };
      return { event: 'fuzz', id: value.id ?? '', name: search.name ?? '', search: searched(search, search.inputs ?? 0) };
    }
    case 'report':
      return {
        event: 'report',
        compiled: value.report?.compiled !== false,
        outcomes: reported(value.report ?? {}),
        profiles: (value.report?.profiles ?? []).map((written) => ({
          test: written.test,
          bench: written.bench ?? undefined,
          file: written.profile,
        })),
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
export const NEEDED = ['json', 'graph', 'test-events', 'test-exact-many', 'test-watch', 'test-coverage', 'lsp-tests', 'explain', 'new', 'test-bench', 'profile', 'test-fuzz'];

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

/** What a reload of a live build did (toolchain 7.10), as `tondo reload
 *  --json` and the debugger's `reload` say it: the functions it changed
 *  and added, how many places move frames to the new code, where frames
 *  stay on the old code and what converting values does; or, when it was
 *  not done, why, and whether the change needs the program started
 *  again. */
export interface Reload {
  changed?: string[];
  added?: string[];
  moves?: number;
  stays?: string[];
  converted?: string[];
  /** What frames passed and runs again as they move, or keeps its value. */
  again?: string[];
  /** Where frames stay on the old code, to mark. */
  stay_lines?: { file: string; lines: number[]; why: string }[];
  message?: string;
  restart?: boolean;
}

export function reloaded(json: string): Reload {
  const said = JSON.parse(json) as Reload & { format?: string };
  if (said.format !== 'tondo-reload-0.1/1') {
    throw new Error(said.message ?? 'tondo reload failed');
  }
  return said;
}

/** A reload as the editor shows it: `applied` when the program runs the
 *  new code, `partly` when some frames may finish on the old code, `same`
 *  when nothing changed, `restart` when the change needs the program
 *  started again and `errors` when the files do not compile; a few words,
 *  and the rest. */
export interface Shown {
  state: 'applied' | 'partly' | 'same' | 'restart' | 'errors';
  text: string;
  details: string[];
}

export function reloadShown(reload: Reload): Shown {
  const why = (reload.message ?? '').replace(/^[^]*not reloaded: /, '');
  if (reload.restart) {
    return { state: 'restart', text: 'restart to apply', details: [why] };
  }
  if (reload.message !== undefined) {
    return {
      state: 'errors',
      text: 'not applied, the files do not compile',
      details: ['The program goes on with its old code; the Problems panel shows the errors.'],
    };
  }
  const changed = reload.changed ?? [];
  const added = reload.added ?? [];
  const names = [...changed, ...added];
  const details = [
    ...(changed.length ? [`Changed: ${changed.join(', ')}.`] : []),
    ...(added.length ? [`Added: ${added.join(', ')}.`] : []),
    ...(reload.moves ? ['Frames on the old code move to the new at their next statement.'] : []),
    ...(reload.converted ?? []),
    ...(reload.again ?? []),
    ...(reload.stays ?? []),
  ];
  if (details.length === 0) {
    return { state: 'same', text: 'nothing changed', details };
  }
  const what = names.length === 0 ? 'the change' : names.length <= 2 ? names.join(', ') : `${names.length} functions`;
  return reload.stays?.length
    ? { state: 'partly', text: `applied ${what}, but some frames keep the old code`, details }
    : { state: 'applied', text: `applied ${what}`, details };
}

/** A search in a line, as `tondo test --fuzz` shows it: its time, inputs
 *  and how many a second, corpus, and the share of the code it reached. */
export function searchLine(name: string, search: Searched): string {
  const seconds = Math.floor(search.elapsedMs / 1000);
  const clock = `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`;
  const count = (n: number) => (n < 10_000 ? `${n}` : n < 1_000_000 ? `${Math.floor(n / 1000)}k` : `${(n / 1e6).toFixed(1)}M`);
  const rate = search.elapsedMs > 0 ? Math.floor((search.inputs * 1000) / search.elapsedMs) : 0;
  const coverage = search.total > 0 ? `  coverage ${Math.floor((search.reached * 100) / search.total)}%` : '';
  return `${name}  ${clock}  ${count(search.inputs)} inputs  ${count(rate)}/s  corpus ${search.corpus}${coverage}`;
}

/** What a test is, as the editor marks it: a benchmark, a fuzz test, or
 *  a plain test. */
export function kindMark(node: { bench?: boolean; fuzz?: boolean; system?: string }): string {
  if (node.system) {
    return `runs on ${node.system}`;
  }
  return node.fuzz ? '🔎 fuzz' : node.bench ? '⏱ benchmark' : '';
}

/** The badge of a file of a system (language 6.8): `watch_linux.to`,
 *  `watch_macos.to` or `signals_unix.to`, and the same before `_test.to`;
 *  none for a common file. */
export function systemBadge(file: string): { badge: string; tooltip: string } | undefined {
  const found = /_(linux|macos|unix)(_test)?\.to$/.exec(file);
  switch (found?.[1]) {
    case 'linux':
      return { badge: 'L', tooltip: 'Linux only: tondo checks it, and builds it for Linux' };
    case 'macos':
      return { badge: 'M', tooltip: 'macOS only: tondo checks it, and builds it for macOS' };
    case 'unix':
      return { badge: 'LM', tooltip: 'Linux and macOS: tondo checks it, and builds it for both' };
    default:
      return undefined;
  }
}

/** The system a target of `tondo version` runs: `x86_64-linux-gnu` is
 *  Linux and `aarch64-macos` macOS. */
export function systemOfTarget(target: string): string {
  return target.includes('macos') ? 'macOS' : 'Linux';
}

/** A page of a value's parts as the debugger's `table` request gives it:
 *  a row for each part, a cell for each field, and the stop it is of. */
export interface Table {
  columns: string[];
  rows: { name: string; cells: Record<string, string> }[];
  start: number;
  total: number;
  stop: number;
}

/** The page that shows the table of the value `title`: its rows sort by a
 *  column's header and filter by what they contain, and the next page of
 *  parts comes when it asks, as a message with where it starts. */
export function tableHtml(title: string, table: Table, nonce: string): string {
  const data = JSON.stringify({ title, ...table }).replace(/</g, '\\u003c');
  return `<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; script-src 'nonce-${nonce}'">
<style>
  body { font-family: var(--vscode-editor-font-family); font-size: var(--vscode-editor-font-size); padding: 0 8px; }
  .bar { position: sticky; top: 0; display: flex; gap: 8px; align-items: center; padding: 6px 0; background: var(--vscode-editor-background); }
  input { flex: 1; background: var(--vscode-input-background); color: var(--vscode-input-foreground); border: 1px solid var(--vscode-input-border, transparent); padding: 3px 6px; }
  button { background: var(--vscode-button-background); color: var(--vscode-button-foreground); border: none; padding: 4px 10px; cursor: pointer; }
  table { border-collapse: collapse; width: 100%; }
  th { position: sticky; top: 34px; background: var(--vscode-editor-background); text-align: left; cursor: pointer; user-select: none; }
  th, td { border-bottom: 1px solid var(--vscode-panel-border); padding: 3px 8px; white-space: pre; }
  td:first-child { color: var(--vscode-descriptionForeground); }
  tr:hover td { background: var(--vscode-list-hoverBackground); }
</style>
</head>
<body>
<div class="bar"><input id="filter" placeholder="Filter the rows"><span id="count"></span><button id="more">Load more</button></div>
<table><thead><tr id="head"></tr></thead><tbody id="rows"></tbody></table>
<script nonce="${nonce}">
const vscode = acquireVsCodeApi();
const table = ${data};
const names = ['', ...table.columns];
let sorted = { column: -1, down: false };
const cell = (row, i) => (i === 0 ? row.name : row.cells[names[i]] ?? '');
const number = (text) => (text.trim() !== '' && !Number.isNaN(Number(text)) ? Number(text) : undefined);
function compare(x, y) {
  const [m, n] = [number(x), number(y)];
  return m !== undefined && n !== undefined ? m - n : x.localeCompare(y, undefined, { numeric: true });
}
function render() {
  document.getElementById('head').replaceChildren(...names.map((name, i) => {
    const th = document.createElement('th');
    th.textContent = name + (sorted.column === i ? (sorted.down ? ' ▼' : ' ▲') : '');
    th.onclick = () => { sorted = { column: i, down: sorted.column === i && !sorted.down }; render(); };
    return th;
  }));
  const wanted = document.getElementById('filter').value.toLowerCase();
  let rows = table.rows.filter((row) => names.some((_, i) => cell(row, i).toLowerCase().includes(wanted)));
  if (sorted.column >= 0) {
    rows = rows.sort((a, b) => (sorted.down ? -1 : 1) * compare(cell(a, sorted.column), cell(b, sorted.column)));
  }
  document.getElementById('rows').replaceChildren(...rows.map((row) => {
    const tr = document.createElement('tr');
    tr.append(...names.map((_, i) => { const td = document.createElement('td'); td.textContent = cell(row, i); return td; }));
    return tr;
  }));
  document.getElementById('count').textContent = \`\${rows.length} shown, \${table.rows.length} of \${table.total} loaded\`;
  document.getElementById('more').hidden = table.start + table.rows.length >= table.total;
}
document.getElementById('filter').oninput = render;
document.getElementById('more').onclick = () =>
  vscode.postMessage({ start: table.start + table.rows.length, stop: table.stop });
window.addEventListener('message', (event) => {
  for (const column of event.data.columns) {
    if (!names.includes(column)) {
      names.push(column);
    }
  }
  table.rows.push(...event.data.rows);
  render();
});
render();
</script>
</body>
</html>
`;
}

/** A task as the debugger lists it: where it is, why it stands still, and
 *  what it waits for and the tasks that hold that. */
export interface Task {
  task: number;
  at: string;
  why: string;
  for?: string;
  on?: number[];
}

/** The tasks as a Mermaid flowchart. */
export function mermaid(tasks: Task[]): string {
  const text = (value: string) => value.replace(/"/g, '#quot;');
  const lines = ['flowchart LR'];
  for (const task of tasks) {
    const waits = task.for ? `<br/>waits for ${text(task.for)}` : `<br/>${text(task.why)}`;
    lines.push(`  t${task.task}["task ${task.task}<br/>${text(shortPlace(task.at))}${waits}"]`);
  }
  for (const task of tasks) {
    for (const holder of task.on ?? []) {
      lines.push(`  t${task.task} -->|waits for| t${holder}`);
    }
  }
  return `${lines.join('\n')}\n`;
}

/** `file:line in function`, with the file's name only. */
export function shortPlace(at: string): string {
  const [where, function_] = at.split(' in ');
  return `${path.basename(where)}${function_ ? ` in ${function_}` : ''}`;
}

/** Each value an expression took over a run, with the line that gave it
 *  and, in a replay, its probe; the probe the program is at, and those of
 *  the checkpoints. */
export interface Timeline {
  changes: { value: string; at: string; tick: number | null }[];
  tick: number;
  checkpoints: number[];
}

/** A page with the timeline of `title`: in a replay, a bar of the run with
 *  each change, where the program is and the checkpoints, and each change
 *  goes to its probe when clicked; a list of the changes in any session.
 *  The data reaches its script as JSON no value can end. */
export function timelineHtml(title: string, timeline: Timeline, nonce: string): string {
  const data = JSON.stringify({ title, ...timeline }).replace(/</g, '\\u003c');
  return `<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; script-src 'nonce-${nonce}'">
<style>
  body { font-family: var(--vscode-editor-font-family); font-size: var(--vscode-editor-font-size); padding: 0 8px; }
  #run { position: relative; height: 36px; margin: 12px 0; border-bottom: 2px solid var(--vscode-panel-border); }
  .mark { position: absolute; bottom: 0; width: 3px; height: 22px; background: var(--vscode-charts-blue); cursor: pointer; }
  .kept { position: absolute; bottom: 0; width: 1px; height: 8px; background: var(--vscode-descriptionForeground); }
  #now { position: absolute; bottom: 0; width: 2px; height: 34px; background: var(--vscode-charts-orange); }
  li { padding: 2px 0; white-space: pre; }
  li.seek { cursor: pointer; }
  li.seek:hover { background: var(--vscode-list-hoverBackground); }
  .at { color: var(--vscode-descriptionForeground); }
</style>
</head>
<body>
<h3 id="title"></h3>
<div id="run"></div>
<ol id="changes"></ol>
<script nonce="${nonce}">
const vscode = acquireVsCodeApi();
const timeline = ${data};
document.getElementById('title').textContent = timeline.title;
const ticks = timeline.changes.map((c) => c.tick).filter((t) => typeof t === 'number' && t > 0);
const end = Math.max(timeline.tick, ...ticks, ...timeline.checkpoints, 1);
const left = (tick) => (100 * tick) / end + '%';
const seek = (tick) => vscode.postMessage({ seek: tick });
function render() {
  const run = document.getElementById('run');
  run.hidden = ticks.length === 0;
  run.replaceChildren(
    ...timeline.checkpoints.map((tick) => Object.assign(document.createElement('div'), { className: 'kept', title: 'checkpoint at probe ' + tick, style: 'left:' + left(tick) })),
    ...timeline.changes.filter((c) => c.tick > 0).map((c) => {
      const mark = Object.assign(document.createElement('div'), { className: 'mark', title: c.value + ' at ' + c.at, style: 'left:' + left(c.tick) });
      mark.onclick = () => seek(c.tick);
      return mark;
    }),
    Object.assign(document.createElement('div'), { id: 'now', title: 'the program is at probe ' + timeline.tick, style: 'left:' + left(timeline.tick) }),
  );
  document.getElementById('changes').replaceChildren(...timeline.changes.map((c) => {
    const item = document.createElement('li');
    const at = Object.assign(document.createElement('span'), { className: 'at', textContent: '  ' + c.at + (c.tick > 0 ? ' (probe ' + c.tick + ')' : '') });
    item.append(c.value, at);
    if (c.tick > 0) {
      item.className = 'seek';
      item.onclick = () => seek(c.tick);
    }
    return item;
  }));
}
window.addEventListener('message', (event) => {
  timeline.tick = event.data.tick;
  render();
});
render();
</script>
</body>
</html>
`;
}

/** A cause `origins` found: a binding, its value, the line that gave it,
 *  and that line's own causes. */
export interface Origin {
  name: string;
  value: string;
  at: string;
  tick?: number;
  above?: boolean;
  causes?: Origin[];
}

/** Where a value came from, as Markdown: the line, then each cause under
 *  the one it explains. */
export function originsMarkdown(title: string, origins: { why: string; at: string; causes: Origin[] }): string {
  const lines = [`# Where ${title} comes from`, '', `${origins.why}, at \`${origins.at}\``, '', ...causeLines(origins.causes)];
  return `${lines.join('\n')}\n`;
}

/** Each cause under the one it explains. */
function causeLines(causes: Origin[], depth = 0): string[] {
  return causes.flatMap((cause) => [
    `${'  '.repeat(depth)}- \`${cause.name}\` was \`${cause.value}\`, from \`${cause.at}\`${cause.above ? ', as above' : ''}`,
    ...(cause.above ? [] : causeLines(cause.causes ?? [], depth + 1)),
  ]);
}

/** What changed between two moments of a replay, as `diff` gives it. */
export interface Diff {
  from: number;
  to: number;
  bindings: { frame: string; name: string; before: string | null; after: string | null }[];
  started: { task: number; at: string }[];
  ended: { task: number; at: string }[];
  memory: { at: string; objects: number; bytes: number }[];
}

export interface Compared {
  parted: boolean;
  task?: number;
  after?: string | null;
  first?: { at: string | null; tick: number | null };
  second?: { at: string | null; tick: number | null };
  bindings?: { frame: string; name: string; before: string | null; after: string | null; read?: boolean }[];
  origins?: { name: string; why: string; at: string; causes: Origin[] }[];
}

/** Where two recordings part, as Markdown: the lines each goes on to, what
 *  differs there, and where that came from. */
export function comparedMarkdown(found: Compared, first: string, second: string): string {
  if (!found.parted) {
    return `# ${first} and ${second}\n\nThe two runs went through the same lines.\n`;
  }
  const next = (run: string, to?: { at: string | null; tick: number | null }) =>
    to?.at ? `- ${run} goes on at \`${to.at}\` (probe ${to.tick})` : `- ${run} ends the task there`;
  const lines = [
    `# ${first} and ${second} part in task ${found.task}`,
    '',
    ...(found.after ? [`After \`${found.after}\`:`, ''] : []),
    next(first, found.first),
    next(second, found.second),
  ];
  const list = (title: string, read: boolean) => {
    const chosen = (found.bindings ?? []).filter((change) => (change.read === true) === read);
    if (chosen.length > 0) {
      lines.push('', `## ${title}`, '');
    }
    for (const change of chosen) {
      lines.push(`- \`${change.name}\`: \`${change.before ?? 'nothing'}\` → \`${change.after ?? 'nothing'}\``);
    }
  };
  list('What that line reads differs', true);
  list('What also differs, which that line does not read', false);
  for (const { name, why, at, causes } of found.origins ?? []) {
    lines.push('', `## Where the \`${name}\` of ${second} came from`, '', `${why}, at \`${at}\``, '', ...causeLines(causes));
  }
  return `${lines.join('\n')}\n`;
}

/** What changed between two moments, as Markdown: the bindings frame by
 *  frame, the tasks, and the memory by where it was made. */
export function diffMarkdown(diff: Diff): string {
  const lines = [`# From probe ${diff.from} to probe ${diff.to}`, ''];
  let frame = '';
  for (const change of diff.bindings) {
    if (change.frame !== frame) {
      frame = change.frame;
      lines.push(`## ${frame}`, '');
    }
    lines.push(`- \`${change.name}\`: \`${change.before ?? 'nothing'}\` → \`${change.after ?? 'nothing'}\``);
  }
  const sign = (n: number) => (n > 0 ? `+${n}` : `${n}`);
  const rest = [
    ...diff.started.map((task) => `- task ${task.task} started, now at \`${task.at}\``),
    ...diff.ended.map((task) => `- task ${task.task} ended, last at \`${task.at}\``),
    ...diff.memory.map((place) => `- memory made at \`${place.at}\`: ${sign(place.objects)} objects, ${sign(place.bytes)} bytes`),
  ];
  if (rest.length > 0) {
    lines.push('', '## Tasks and memory', '', ...rest);
  }
  return `${lines.join('\n')}\n`;
}

/** What a launch with `last` runs: the program recorded, keeping the end
 *  of the run. */
export function recordedRun(config: Record<string, unknown>): string[] {
  const hold = config.hold ? ['--hold', String(config.hold)] : [];
  const args = Array.isArray(config.args) ? config.args.map(String) : [];
  const recording = String(config.recording ?? '.tondo/recording');
  return ['run', '--record', recording, '--last', String(config.last), ...hold, String(config.program), '--', ...args];
}

/** The page of `tondo dump report` for a webview: only its own script runs,
 *  which sends each place clicked to the editor. */
export function dumpPage(html: string, nonce: string): string {
  const policy = `<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; script-src 'nonce-${nonce}'">`;
  const script = `<script nonce="${nonce}">
const vscode = acquireVsCodeApi();
document.addEventListener('click', (event) => {
  const link = event.target.closest('a[data-file]');
  if (link) {
    event.preventDefault();
    vscode.postMessage({ file: link.dataset.file, line: Number(link.dataset.line) });
  }
});
</script>`;
  return html.replace('<head>', `<head>${policy}`).replace('</body>', `${script}</body>`);
}

/** What `tondo logpoint` runs for a line of a running program. */
export function logpointArgs(
  pid: string,
  place: string,
  text: string,
  condition: string,
  max: number,
  dump = '',
): string[] {
  const shown = text ? [text] : [];
  const only = condition ? ['--if', condition] : [];
  const whole = dump ? ['--dump', dump] : [];
  return ['logpoint', pid, place, ...shown, ...only, ...whole, '--max', String(max)];
}

/** A declaration of `tondo api --diff --json` that changed. */
export interface ApiChange {
  name: string;
  was: string;
  file: string;
  line: number;
}

/** Where each change of the public API goes in the editor. */
export function apiMarks(changes: ApiChange[]): { file: string; line: number; text: string }[] {
  return changes.map(({ name, was, file, line }) => ({ file, line, text: `\`${name}\` was \`${was}\`: this breaks who uses it` }));
}

/** A failure of `tondo dump group --json`. */
export interface Failure {
  count: number;
  at?: string | null;
  message?: string | null;
  first_commit?: string | null;
  lock_orders?: string[];
}

/** Each line an order of locks that can deadlock names, with what to say
 *  there. */
function orderMarks(failure: Failure): { file: string; line: number; text: string }[] {
  return (failure.lock_orders ?? []).flatMap((order) =>
    [...order.matchAll(/([^\s,]+\.to):(\d+)/g)]
      .filter((found) => !found[1].startsWith('<'))
      .map((found) => ({ file: found[1], line: Number(found[2]), text: order })),
  );
}

/** Where each failure of a group of dumps goes in the editor: the line of
 *  the program it fell at, and what to say there; and the lines of the
 *  orders of locks that can deadlock, which did not fall yet. */
export function failureMarks(failures: Failure[]): { file: string; line: number; text: string }[] {
  return failures.flatMap((failure) => {
    const orders = orderMarks(failure);
    const found = /^(.*):(\d+) in /.exec(failure.at ?? '');
    if (!found || found[1].startsWith('<')) {
      return orders;
    }
    const times = failure.count === 1 ? 'once' : `${failure.count} times`;
    const since = failure.first_commit ? `, first at ${failure.first_commit.slice(0, 10)}` : '';
    return [...orders, { file: found[1], line: Number(found[2]), text: `Fell here ${times}${since}: ${failure.message ?? ''}` }];
  });
}

/** A step of a request's timeline, as `tondo requests` gives it. */
export interface RequestStep {
  ms: number;
  task: number;
  what: string;
  other?: number;
  on?: string;
  took_ms?: number;
  text?: string;
  at?: string | null;
}

export interface Flight {
  id: number;
  flying?: boolean;
  /** The timeline as a picture, in SVG. */
  picture?: string;
  method: string;
  path: string;
  ms: number;
  task: number;
  steps: RequestStep[];
}

export interface Requests {
  requests: Flight[];
  served: Flight[];
}

/** A row of the view of requests: what it says, and the place it opens. */
export interface RequestRow {
  label: string;
  description: string;
  place?: { file: string; line: number };
}

/** A request as its row, in flight or answered. */
export function requestRow(request: Flight): RequestRow {
  const tasks = new Set(request.steps.map((step) => step.task)).size;
  const through = `${tasks} task${tasks === 1 ? '' : 's'}`;
  const took = request.flying ? `for ${request.ms} ms, in flight` : `${request.ms} ms`;
  return { label: `${request.method} ${request.path}`, description: `${took}, ${through}` };
}

/** A step as its row, at how long after the request came. */
export function stepRow(step: RequestStep): RequestRow {
  const task = `task ${step.task}`;
  const said: Record<string, string> = {
    took: `${task} takes it`,
    spawned: `${task} starts task ${step.other}`,
    waited: `${task} waited ${step.took_ms} ms for ${step.on}`,
    ended: `${task} ended`,
    panicked: `${task} panicked: ${step.text}`,
  };
  const at = step.at?.match(/^(.*):(\d+) in /);
  const place = at ? { file: at[1], line: Number(at[2]) } : undefined;
  return { label: `${step.ms} ms  ${said[step.what] ?? step.what}`, description: step.at ?? '', place };
}

/** Where a warning of a test says the write that loses an update is. */
export function warningPlace(warning: string): { file: string; line: number } | undefined {
  const at = warning.match(/ wrote it at (.*?):(\d+) in /);
  return at ? { file: at[1], line: Number(at[2]) } : undefined;
}

/** What a line left, as the editor shows it after the line: the bindings the
 *  line names. */
export function lineValues(bindings: { name: string; value: string }[], text: string): string {
  const named = bindings.filter(({ name }) => new RegExp(`\\b${name}\\b`).test(text));
  const shown = named.map(({ name, value }) => `${name} = ${value}`).join('  ');
  return shown.length > 100 ? `${shown.slice(0, 100)}…` : shown;
}

/** A task a panic holds in a live build, until a change is saved. */
export interface HeldPanic {
  task: number;
  at: string;
  message: string;
}

/** Where a held panic is, from `file:line in function`. */
export function heldPlace(held: HeldPanic): { file: string; line: number } | undefined {
  const at = held.at.match(/^(.*):(\d+) in /);
  return at ? { file: at[1], line: Number(at[2]) } : undefined;
}

/** What a hover says of a name, from what the lines of its file left: the
 *  nearest line above that shows it, or any. */
export function liveHover(lines: Record<string, { name: string; value: string }[]>, name: string, line: number): string | undefined {
  const shown = Object.entries(lines)
    .filter(([, bindings]) => bindings.some((binding) => binding.name === name))
    .sort(([a], [b]) => Math.abs(Number(a) - line) - Math.abs(Number(b) - line));
  const nearest = shown[0]?.[1].find((binding) => binding.name === name);
  return nearest && `${name} = ${nearest.value}`;
}

/** Whether what a debug session refused needs a live build. */
export function needsLive(message: { type?: string; success?: boolean; message?: string }): boolean {
  return message.type === 'response' && message.success === false && /\blive build\b/.test(message.message ?? '');
}

/** What `tondo api --search` found, to pick from. */
export function apiMatches(text: string): { label: string; description: string; detail: string }[] {
  const found = JSON.parse(text) as { matches: { name: string; module: string; doc?: string }[] };
  return found.matches.map(({ name, module, doc }) => ({ label: name, description: module, detail: doc ?? '' }));
}

/** What `tondo check --unreached` found, to mark on their lines. */
export function unreachedMarks(text: string): { file: string; line: number; text: string }[] {
  const found = JSON.parse(text) as { unreached: { file: string; line: number; name: string }[] };
  return found.unreached.map(({ file, line, name }) => ({ file, line, text: `nothing reaches \`${name}\`` }));
}

/** `tondo add`, from where a dependency comes. */
export function dependencyArgs(alias: string, kind: string, where: string, project: string): string[] {
  return ['add', alias, `--${kind}`, where, '--project', project];
}

/** A command line as a shell splits it: by spaces, but for what quotes hold. */
export function splitArgs(text: string): string[] {
  const words = text.match(/"[^"]*"|'[^']*'|\S+/g) ?? [];
  return words.map((word) => word.replace(/^(["'])(.*)\1$/, '$2'));
}
