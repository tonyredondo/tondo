// What tondo says as JSON, as the extension reads it: the list, the
// events and the report of `tondo test`, `tondo graph` and `tondo version`.

import * as assert from 'node:assert/strict';
import { test } from 'node:test';
import { incompatible, listed, outcomes, place, programOf, testEvent, verdict } from '../report';

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
  assert.equal(incompatible('{"format":"tondo-version-0.1/1","ok":true,"version":"0.1.0-dev"}'), undefined);
  assert.match(incompatible('tondo 0.1.0-dev (native, cranelift)\n') ?? '', /older than the extension.*install\.sh/);
});
