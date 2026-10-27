// Finding `tondo`.

import * as assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { test } from 'node:test';
import { findTondo, misconfigured } from '../binary';

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
  // One configured that is not there is not looked for elsewhere.
  touch(path.join(bin, 'tondo'), 0o755);
  assert.equal(findTondo('/opt/tondo', { PATH: bin }, home), undefined);
  fs.rmSync(path.join(bin, 'tondo'));
  assert.equal(findTondo('', { PATH: bin }, home), undefined);
  touch(path.join(home, '.local', 'bin', 'tondo'), 0o755);
  assert.equal(findTondo('', { PATH: bin }, home), path.join(home, '.local', 'bin', 'tondo'));
  touch(path.join(bin, 'tondo'), 0o755);
  assert.equal(findTondo('', { PATH: bin }, home), path.join(bin, 'tondo'));
});

test('tondo.path may be the file, its directory or a download with its bin, from ~, and says what is wrong', () => {
  const home = scratch();
  const download = path.join(home, 'Downloads', 'tondo-aarch64-macos');
  const tondo = path.join(download, 'bin', 'tondo');
  touch(tondo, 0o755);
  for (const configured of [tondo, path.join(download, 'bin'), download, '~/Downloads/tondo-aarch64-macos/bin/tondo', `"${tondo}"`, ` ${download}/ `]) {
    assert.equal(findTondo(configured, {}, home), tondo, configured);
  }
  assert.equal(findTondo('~/elsewhere', {}, home), undefined);
  assert.match(misconfigured('~/elsewhere', home), /where there is no tondo/);
  fs.chmodSync(tondo, 0o644);
  assert.equal(findTondo(tondo, {}, home), undefined);
  assert.match(misconfigured(download, home), /chmod \+x .*bin\/tondo/);
});
