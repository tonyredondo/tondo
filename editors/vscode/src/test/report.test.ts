// What tondo says, as the extension reads it: the list, the events and
// the report of `tondo test`, `tondo graph`, `tondo check`, `tondo
// doc-test` and `tondo version`.

import * as assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  NEEDED,
  benchLine,
  checked,
  counterRows,
  covered,
  duration,
  examples,
  failures,
  graphed,
  hotLines,
  incompatible,
  listed,
  outcomes,
  place,
  programOf,
  reloadShown,
  reloaded,
  rendered,
  running,
  served,
  share,
  testEvent,
  traced,
  verdict,
} from '../report';

const source = (line: number) => ({ file: 'src/main_test.to', line, column: 6 });

test('a list gives suites first, with their tests under them', () => {
  const list = JSON.stringify({
    format: 'tondo-test-list-0.1/1',
    suites: [{ id: 'p::unit::p::group', parent: null, name: 'group', source: source(9) }],
    tests: [
      { id: 'p::unit::p::adds', parent: null, name: 'adds', source: source(1) },
      { id: 'p::unit::p::group::inner', parent: 'p::unit::p::group', name: 'inner', source: source(10) },
    ],
  });
  assert.deepEqual(
    listed(list).map((n) => [n.id, n.parent, n.suite, n.line]),
    [
      ['p::unit::p::group', null, true, 9],
      ['p::unit::p::adds', null, false, 1],
      ['p::unit::p::group::inner', 'p::unit::p::group', false, 10],
    ],
  );
});

test('a report says how each test ended, why and where', () => {
  const stream = (data: string) => ({ encoding: 'utf8', data });
  const report = JSON.stringify({
    format: 'tondo-test-report-0.1/1',
    suites: [],
    tests: [
      {
        id: 'p::unit::p::adds',
        name: 'adds',
        status: 'passed',
        decisive_attempt: 1,
        attempts: [{ failure: null, skip: null, logs: ['logged'], stdout: stream('out\n'), stderr: stream('') }],
      },
      {
        id: 'p::unit::p::fails',
        name: 'fails',
        status: 'failed-panic',
        decisive_attempt: 1,
        attempts: [
          {
            failure: { kind: 'panic', message: 'assertion-failed: wrong sum', source: { file: 'src/main_test.to', line: 7, column: 5 } },
            skip: null,
            logs: [],
            stdout: { encoding: 'base64', data: Buffer.from('bytes').toString('base64') },
            stderr: stream(''),
          },
        ],
      },
      {
        id: 'p::unit::p::later',
        name: 'later',
        status: 'skipped',
        decisive_attempt: 1,
        attempts: [{ failure: null, skip: { reason: 'not yet', source: source(12) }, logs: [], stdout: stream(''), stderr: stream('') }],
      },
    ],
  });
  const [passed, failed, skipped] = outcomes(report);
  assert.equal(passed.verdict, 'passed');
  assert.equal(passed.output, 'out\nlogged\n');
  assert.equal(failed.verdict, 'failed');
  assert.equal(failed.message, 'assertion-failed: wrong sum');
  assert.deepEqual([failed.file, failed.line, failed.column], ['src/main_test.to', 7, 5]);
  assert.equal(failed.output, 'bytes');
  assert.equal(skipped.verdict, 'skipped');
  assert.equal(skipped.message, 'not yet');
});

test('every status of the report has a verdict', () => {
  assert.equal(verdict('flaky-pass'), 'passed');
  assert.equal(verdict('blocked-skip'), 'skipped');
  assert.equal(verdict('failed-error'), 'failed');
  for (const status of ['timeout', 'crashed', 'blocked-setup', 'resource-limit', 'infrastructure']) {
    assert.equal(verdict(status), 'errored');
  }
});

test('the events of a run say what started and how each attempt ended, then the report', () => {
  const stream = (data: string) => ({ encoding: 'utf8', data });
  assert.deepEqual(testEvent('{"event":"pass","format":"tondo-test-events-0.1/1","iteration":1,"round":0,"tests":["p::a"]}'), {
    event: 'pass',
    tests: ['p::a'],
  });
  assert.deepEqual(testEvent('{"event":"start","id":"p::a","suite":false,"iteration":1,"round":0}'), { event: 'start', id: 'p::a' });
  const end = testEvent(
    JSON.stringify({
      event: 'end',
      id: 'p::a',
      status: 'failed-error',
      duration_ms: 12,
      failure: { kind: 'error', message: 'bad input', source: { file: 'src/a_test.to', line: 3, column: 5 } },
      skip: null,
      logs: [],
      stdout: stream('printed\n'),
      stderr: stream(''),
    }),
  );
  assert.equal(end?.event, 'end');
  if (end?.event === 'end') {
    assert.equal(end.duration, 12);
    assert.deepEqual([end.outcome.verdict, end.outcome.message, end.outcome.line, end.outcome.output], ['failed', 'bad input', 3, 'printed\n']);
  }
  assert.deepEqual(testEvent('{"event":"output","id":"p::a","suite":false,"iteration":1,"round":0,"stdout":{"encoding":"utf8","data":"half"}}'), {
    event: 'output',
    id: 'p::a',
    text: 'half',
  });
  const compared = testEvent(
    JSON.stringify({
      event: 'end',
      id: 'p::b',
      status: 'failed-panic',
      failure: {
        kind: 'panic',
        message: 'assertion-failed: sum\n  condition: sum == 5\n  left:  4\n  right: 5',
        source: { file: 'src/a_test.to', line: 7, column: 5 },
        comparison: { left: '4', right: '5' },
        stack: [
          { function: 'add', source: { file: 'src/a.to', line: 2, column: 5 }, task: 0, times: 1 },
          { function: 'test b', source: { file: 'src/a_test.to', line: 7, column: 5 }, task: 0, times: 1 },
          { function: 'main', source: null, task: 1, times: 1 },
        ],
      },
      logs: ['noted'],
      stdout: stream('x'),
      stderr: stream(''),
    }),
  );
  assert.equal(compared?.event, 'end');
  if (compared?.event === 'end') {
    assert.deepEqual(compared.outcome.comparison, { left: '4', right: '5' });
    assert.deepEqual(
      compared.outcome.stack.map((f) => [f.function, f.file, f.line]),
      [
        ['add', 'src/a.to', 2],
        ['test b', 'src/a_test.to', 7],
      ],
    );
    assert.equal(compared.outcome.logs, 'noted\n');
    assert.equal(compared.outcome.output, 'xnoted\n');
  }
  const report = testEvent(JSON.stringify({ event: 'report', report: { compiled: true, suites: [], tests: [{ id: 'p::a', status: 'flaky-pass', attempts: [] }] } }));
  assert.deepEqual(report?.event === 'report' && [report.compiled, report.outcomes.map((o) => o.verdict)], [true, ['passed']]);
  const uncompiled = testEvent('{"event":"report","report":{"compiled":false,"suites":[],"tests":[]}}');
  assert.equal(uncompiled?.event === 'report' && uncompiled.compiled, false);
  assert.equal(testEvent('warning: not json'), undefined);
});

test('a file runs as its project, or alone, as tondo graph says', () => {
  const inProject = place('{"format":"tondo-graph-0.1/1","ok":true,"root":"/w/app","project":true,"focus":"app.models","packages":[]}');
  assert.deepEqual(inProject, { root: '/w/app', project: true, module: 'app.models' });
  assert.deepEqual(programOf(inProject, '/w/app/src/models/user.to'), { program: '/w/app', cwd: '/w/app' });
  const alone = place('{"format":"tondo-graph-0.1/1","ok":true,"root":"/w/hello.to","project":false,"focus":null,"packages":[]}');
  assert.deepEqual(programOf(alone, '/w/hello.to'), { program: '/w/hello.to', cwd: '/w' });
  assert.throws(() => place('{"format":"tondo-error-0.1/1","ok":false,"message":"no"}'));
});

test('a tondo older than the extension is told to update', () => {
  const version = (features: string[]) => JSON.stringify({ format: 'tondo-version-0.1/1', ok: true, version: '0.1.0-dev', features });
  assert.equal(incompatible(version(NEEDED)), undefined);
  assert.match(incompatible(version(['json'])) ?? '', /older than the extension/);
  assert.match(incompatible('{"format":"tondo-version-0.1/1","ok":true,"version":"0.1.0-dev"}') ?? '', /older/);
  assert.match(incompatible('tondo 0.1.0-dev (native, cranelift)\n') ?? '', /older than the extension.*install\.sh/);
});

test('the tests the language server sends are a list, with each file whole', () => {
  const sent = served({
    root: 'file:///w/app',
    tests: [
      { id: 'app::unit::app::group', parent: null, name: 'group', suite: true, location: { uri: 'file:///w/app/src/main_test.to', range: { start: { line: 4, character: 6 } } } },
      { id: 'app::unit::app::group::inner', parent: 'app::unit::app::group', name: 'inner', suite: false },
    ],
  });
  assert.equal(sent.dir, '/w/app');
  assert.deepEqual(sent.nodes[0], { id: 'app::unit::app::group', parent: null, name: 'group', suite: true, bench: false, file: '/w/app/src/main_test.to', line: 5, column: 7 });
  assert.equal(sent.nodes[1].file, undefined);
});

test('a report of coverage says how many times each line, function and way ran', () => {
  const report = JSON.stringify({
    format: 'tondo-coverage-0.1/1',
    files: [
      {
        file: 'src/main.to',
        lines: [{ line: 10, count: 4 }],
        functions: [{ name: 'area', line: 9, count: 2 }],
        branches: [{ branch: { line: 10, column: 5 }, line: 11, column: 9, label: 'Shape.Circle(r)', count: 1 }],
      },
    ],
    summary: {},
  });
  const [file] = covered(report);
  assert.equal(file.file, 'src/main.to');
  assert.deepEqual(file.branches[0].branch, { line: 10, column: 5 });
  assert.throws(() => covered('{"format":"tondo-test-report-0.1/1"}'));
});

test('running programs, counters, traces and graphs read as the views show them', () => {
  const programs = running('{"format":"tondo-ps-0.1/1","ok":true,"programs":[{"pid":7,"program":"/w/app","arguments":["-v"],"uptime_ms":10}]}');
  assert.deepEqual(programs.map((p) => [p.pid, p.program]), [[7, '/w/app']]);
  assert.throws(() => running('[]'));
  const rows = counterRows(
    '{"pid":7,"uptime_ms":10,"counters":[{"name":"tasks","unit":"","value":3},{"name":"heap","unit":"bytes","value":100,"rate":2.5},{"name":"gc","unit":"us","value":4}]}',
  );
  assert.deepEqual(rows, [
    { name: 'tasks', value: '3' },
    { name: 'heap', value: '100 bytes, 2.5 bytes/s' },
    { name: 'gc', value: '4 µs' },
  ]);
  assert.equal(counterRows('not json'), undefined);
  assert.deepEqual(traced('{"format":"tondo-trace-0.1/1","ok":true,"timeline":"/t/a.json","profile":"/t/a.folded"}'), {
    timeline: '/t/a.json',
    profile: '/t/a.folded',
  });
  assert.throws(() => traced('{"format":"tondo-error-0.1/1","ok":false,"message":"no program app"}'), /no program app/);
  const packages = graphed(
    JSON.stringify({
      format: 'tondo-graph-0.1/1',
      packages: [{ name: 'app', dir: '/w/app' }],
      modules: [
        { name: 'app', package: 'app', files: ['/w/app/src/main.to'], imports: [{ module: 'std.json', file: '/w/app/src/main.to', line: 1 }] },
        { name: 'std.json', package: 'std', files: [], imports: [] },
      ],
    }),
  );
  assert.deepEqual(packages.map((p) => [p.name, p.modules.map((m) => m.name)]), [
    ['app', ['app']],
    ['std', ['std.json']],
  ]);
});

test('a check gives its diagnostics, counted from 0, and the terminal shows them from 1', () => {
  const report = checked(
    JSON.stringify({
      format: 'tondo-check-0.1/1',
      ok: false,
      diagnostics: [
        {
          code: 'E1001',
          file: 'src/main.to',
          message: 'unknown name `foo`',
          range: { start: { byte: 16, line: 1, column: 4 }, end: { byte: 19, line: 1, column: 7 } },
          severity: 'error',
        },
      ],
    }),
  );
  assert.equal(report.ok, false);
  assert.deepEqual(report.problems[0].start, { line: 1, column: 4 });
  assert.deepEqual(report.problems[0].end, { line: 1, column: 7 });
  assert.equal(rendered(report.problems[0]), 'error[E1001]: unknown name `foo`\n  --> src/main.to:2:5');
  // A build says what it built; an error that is no diagnostic is one.
  assert.equal(checked('{"format":"tondo-build-0.1/1","ok":true,"output":"app","diagnostics":[]}').output, 'app');
  const broken = checked('{"format":"tondo-error-0.1/1","ok":false,"message":"tondo.toml: line 1: expected `key = value`"}');
  assert.equal(rendered(broken.problems[0]), 'error: tondo.toml: line 1: expected `key = value`');
  assert.throws(() => checked('{}'));
});

test("a document's examples, and what doc-test says of those that fail", () => {
  const text = '# Doc\n\n~~~tondo\nfn main() {}\n~~~\n\n~~~tondo compile-fail E1102\nfn main() {}\r\n~~~\n';
  assert.deepEqual(examples(text), [
    { line: 2, header: 'tondo' },
    { line: 6, header: 'tondo compile-fail E1102' },
  ]);
  const failed = failures('doc.md:7: this example should fail with E1102, and it compiles\n  more\n/w/b.md:3: does not parse\n');
  assert.deepEqual([...failed], [
    [7, 'this example should fail with E1102, and it compiles\n  more'],
    [3, 'does not parse'],
  ]);
});

test('benchmarks are marked, and what they measured reads as tondo shows it', () => {
  const list = JSON.stringify({
    format: 'tondo-test-list-0.1/1',
    suites: [],
    tests: [
      { id: 'p::unit::p::adds', parent: null, name: 'adds', bench: false },
      { id: 'p::unit::p::parsing', parent: null, name: 'parsing', bench: true },
    ],
  });
  assert.deepEqual(
    listed(list).map((n) => [n.name, n.bench]),
    [
      ['adds', false],
      ['parsing', true],
    ],
  );
  const benchmark = {
    name: 'parse 10',
    ns_per_op: { median: 1973.4, deviation: 9.9 },
    allocations_per_op: 51,
    bytes_per_op: 1685,
    baseline: { change_percent: 7.443, verdict: 'slower', regressed: '7.4% slower' },
  };
  const ended = testEvent(JSON.stringify({ event: 'end', id: 'p::unit::p::parsing', status: 'passed', benchmarks: [benchmark] }));
  assert.ok(ended?.event === 'end');
  const [measured] = ended.outcome.benchmarks;
  assert.equal(benchLine(measured), 'parse 10  1.973 µs/op  ±0.5%  51 allocs/op  1685 B/op  +7.4% slower, regressed: 7.4% slower');
  assert.equal(benchLine({ ...measured, change: undefined, allocations: 0.5 }), 'parse 10  1.973 µs/op  ±0.5%  0.50 allocs/op  1685 B/op');
  assert.equal(duration(152.24), '152.2 ns');
  assert.equal(duration(2.5e9), '2.500 s');
});

test("a profile's hot lines, and how much each took at its end", () => {
  const shown = JSON.stringify({
    format: 'tondo-profile-show-0.1/1',
    type: 'cpu',
    unit: 'nanoseconds',
    total: 146_000_000,
    lines: [
      { file: 'busy.to', line: 14, flat: 94_000_000, cum: 94_000_000 },
      { file: 'busy.to', line: 23, flat: 2_000_000, cum: 141_000_000 },
    ],
  });
  const hot = hotLines(shown);
  assert.equal(hot.lines.length, 2);
  assert.equal(share(hot.lines[0].flat, hot.lines[0].cum, hot.total, hot.unit), '🔥 64.4% (94ms)');
  assert.equal(share(hot.lines[1].flat, hot.lines[1].cum, hot.total, hot.unit), '🔥 1.4% (2ms), 96.6% with what it calls');
  assert.equal(share(100, 100, hot.total, hot.unit), undefined);
  assert.equal(share(3 << 20, 3 << 20, 4 << 20, 'bytes'), '🔥 75.0% (3.0MiB)');
  assert.throws(() => hotLines('{"format":"tondo-error-0.1/1"}'));
  const report = testEvent(JSON.stringify({ event: 'report', report: { compiled: true, profiles: [{ test: 'p::unit::p::slow', bench: null, profile: 'target/profiles/p.unit.p.slow.cpu.pprof' }] } }));
  assert.ok(report?.event === 'report');
  assert.deepEqual(report.profiles, ['target/profiles/p.unit.p.slow.cpu.pprof']);
});

test('a reload says whether the program runs the new code, or why not', () => {
  const applied = reloadShown(reloaded('{"format":"tondo-reload-0.1/1","ok":true,"changed":["main","reply"],"added":[],"moves":2,"stays":[],"converted":[]}'));
  assert.equal(applied.state, 'applied');
  assert.equal(applied.text, 'applied main, reply');
  assert.deepEqual(applied.details, ['Changed: main, reply.', 'Frames on the old code move to the new at their next statement.']);
  const partly = reloadShown({ changed: ['a', 'b'], added: ['c'], stays: ['frames of `a` finish on the old code: its parameters or result changed'] });
  assert.equal(partly.state, 'partly');
  assert.equal(partly.text, 'applied 3 functions, but some frames keep the old code');
  assert.equal(reloadShown({ changed: [], added: [], moves: 0, stays: [], converted: [] }).state, 'same');
  const restart = reloadShown(
    reloaded('{"format":"tondo-reload-0.1/1","ok":false,"message":"not reloaded: `Mode` changed what it is made of: start the program again","restart":true}'),
  );
  assert.equal(restart.state, 'restart');
  assert.deepEqual(restart.details, ['`Mode` changed what it is made of: start the program again']);
  const errors = reloadShown({ message: "error[E1001]: ...\nnot reloaded: the program goes on as it was", restart: false });
  assert.equal(errors.state, 'errors');
  assert.throws(() => reloaded('{"format":"tondo-error-0.1/1","ok":false,"message":"no program 42 runs"}'), /no program 42 runs/);
});
