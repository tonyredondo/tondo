// Running `tondo` and reading what it says.

import { spawn } from 'node:child_process';
import * as path from 'node:path';
import * as vscode from 'vscode';
import { tondo } from './extension';
import { Place, place, programOf } from './report';

/** What `tondo` printed, and how it ended. */
export interface Ran {
  code: number | null;
  stdout: string;
  stderr: string;
}

/** Runs `tondo`; `line` sees each line of its output as it comes, and
 *  `env` adds to its environment. */
export function exec(
  args: string[],
  cwd: string,
  token?: vscode.CancellationToken,
  line?: (text: string) => void,
  env?: Record<string, string>,
): Promise<Ran> {
  return new Promise((resolve, reject) => {
    const child = spawn(tondo(), args, { cwd, env: env && { ...process.env, ...env } });
    let stdout = '';
    let stderr = '';
    let pending = '';
    child.stdout.on('data', (data: Buffer) => {
      const text = data.toString();
      stdout += text;
      if (line) {
        const lines = (pending + text).split('\n');
        pending = lines.pop() ?? '';
        lines.forEach(line);
      }
    });
    child.stderr.on('data', (data: Buffer) => (stderr += data.toString()));
    child.on('error', reject);
    child.on('close', (code) => {
      if (line && pending) {
        line(pending);
      }
      resolve({ code, stdout, stderr });
    });
    token?.onCancellationRequested(() => child.kill());
  });
}

/** What the settings give the programs or the tests the editor runs, for
 *  the folder of a path: `tondo.run.args` and `tondo.run.env`, or
 *  `tondo.test.args` and `tondo.test.env`. */
export function settings(kind: 'run' | 'test', at?: string): { args: string[]; env: Record<string, string> } {
  const config = vscode.workspace.getConfiguration('tondo', at ? vscode.Uri.file(at) : undefined);
  return { args: config.get<string[]>(`${kind}.args`, []), env: config.get<Record<string, string>>(`${kind}.env`, {}) };
}

/** Where a file or directory is: its project, and its module. */
export async function placeOf(file: string): Promise<Place> {
  const ran = await exec(['graph', file, '--packages', '--json'], path.dirname(file));
  try {
    return place(ran.stdout);
  } catch {
    throw new Error(ran.stderr.trim() || `tondo graph ${file} failed`);
  }
}

/** What to build to run or debug a file: its project, or the file. */
export async function programFor(file: string): Promise<{ program: string; cwd: string }> {
  return programOf(await placeOf(file), file);
}
