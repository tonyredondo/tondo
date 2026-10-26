// Where `tondo` is, and which project a file belongs to. Nothing here
// needs VS Code, so its tests run with Node alone.

import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';

/** The `tondo` executable: the one configured, one on `PATH`, or the one
 *  `dist/install.sh` puts in `~/.cargo/bin` or `~/.local/bin`, which an
 *  editor started from the desktop may not have on its `PATH`. */
export function findTondo(configured: string, env: NodeJS.ProcessEnv = process.env, home = os.homedir()): string | undefined {
  if (configured) {
    return configured;
  }
  const name = process.platform === 'win32' ? 'tondo.exe' : 'tondo';
  const dirs = (env.PATH ?? '').split(path.delimiter).filter((dir) => dir !== '');
  dirs.push(path.join(home, '.cargo', 'bin'), path.join(home, '.local', 'bin'));
  return dirs.map((dir) => path.join(dir, name)).find(isExecutable);
}

function isExecutable(file: string): boolean {
  try {
    fs.accessSync(file, fs.constants.X_OK);
    return fs.statSync(file).isFile();
  } catch {
    return false;
  }
}

/** Whether a directory is a project: it has `tondo.toml`, `src/main.to`
 *  or `main.to`. */
export function isProject(dir: string): boolean {
  return ['tondo.toml', path.join('src', 'main.to'), 'main.to'].some((file) => fs.existsSync(path.join(dir, file)));
}

/** The project a file belongs to, the nearest above it, or `undefined`
 *  for a file on its own, as a script. */
export function projectOf(file: string): string | undefined {
  let dir = path.dirname(file);
  for (;;) {
    if (isProject(dir)) {
      return dir;
    }
    const parent = path.dirname(dir);
    if (parent === dir) {
      return undefined;
    }
    dir = parent;
  }
}

/** What to build to run or debug a file: its project, or the file. */
export function programOf(file: string): { program: string; cwd: string } {
  const project = projectOf(file);
  return project ? { program: project, cwd: project } : { program: file, cwd: path.dirname(file) };
}
