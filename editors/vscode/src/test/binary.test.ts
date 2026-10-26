// Finding `tondo`, and the project a file belongs to.

import * as assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { test } from 'node:test';
import { findTondo, programOf, projectOf } from '../binary';

function scratch(): string {
  return fs.mkdtempSync(path.join(os.tmpdir(), 'tondo-vscode-'));
}

function touch(file: string, mode = 0o644): void {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, '');
  fs.chmodSync(file, mode);
}

test('tondo is the one configured, then one on PATH, then where the installer puts it', () => {
  const home = scratch();
  const bin = path.join(home, 'bin');
  assert.equal(findTondo('/opt/tondo', { PATH: bin }, home), '/opt/tondo');
  assert.equal(findTondo('', { PATH: bin }, home), undefined);
  touch(path.join(home, '.local', 'bin', 'tondo'), 0o755);
  assert.equal(findTondo('', { PATH: bin }, home), path.join(home, '.local', 'bin', 'tondo'));
  touch(path.join(bin, 'tondo'), 0o755);
  assert.equal(findTondo('', { PATH: bin }, home), path.join(bin, 'tondo'));
});

test('a file runs as its project, or alone', () => {
  const root = scratch();
  touch(path.join(root, 'app', 'tondo.toml'));
  touch(path.join(root, 'app', 'src', 'parse.to'));
  assert.equal(projectOf(path.join(root, 'app', 'src', 'parse.to')), path.join(root, 'app'));
  touch(path.join(root, 'scripts', 'hello.to'));
  assert.equal(projectOf(path.join(root, 'scripts', 'hello.to')), undefined);
  assert.deepEqual(programOf(path.join(root, 'scripts', 'hello.to')), {
    program: path.join(root, 'scripts', 'hello.to'),
    cwd: path.join(root, 'scripts'),
  });
});
