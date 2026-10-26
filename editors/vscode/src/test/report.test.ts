// The list and the report of `tondo test`, as the Testing view reads them.

import * as assert from 'node:assert/strict';
import { test } from 'node:test';
import { listed, outcomes, verdict } from '../report';

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
