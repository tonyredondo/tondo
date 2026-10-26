// The tests `tondo test --list --test-format json` lists, and what the
// report of `tondo test --test-format json` says of each (testing 15).

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
  /** What it wrote, and what it logged. */
  output: string;
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
  failure?: { message?: string; source?: Source } | null;
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

/** How each suite and test of a report ended. */
export function outcomes(json: string): Outcome[] {
  const report = JSON.parse(json) as { suites?: Entry[]; tests?: Entry[] };
  return [...(report.suites ?? []), ...(report.tests ?? [])].map((entry) => {
    const attempts = entry.attempts ?? [];
    const attempt = attempts[(entry.decisive_attempt ?? attempts.length) - 1] ?? attempts[attempts.length - 1];
    const why = attempt?.failure ?? attempt?.skip ?? undefined;
    const message = attempt?.failure?.message ?? attempt?.skip?.reason;
    const output = [text(attempt?.stdout), text(attempt?.stderr), ...(attempt?.logs ?? []).map((log) => `${log}\n`)].join('');
    return {
      id: entry.id,
      verdict: verdict(entry.status ?? ''),
      message,
      file: why?.source?.file,
      line: why?.source?.line,
      column: why?.source?.column,
      output,
    };
  });
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
