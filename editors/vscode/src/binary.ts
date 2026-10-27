// Where `tondo` is, and whether a directory is a project. Nothing here
// needs VS Code, so its tests run with Node alone.

import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';

const NAME = process.platform === 'win32' ? 'tondo.exe' : 'tondo';

/** The `tondo` executable: the one configured, one on `PATH`, or the one
 *  `dist/install.sh` puts in `~/.cargo/bin` or `~/.local/bin`, which an
 *  editor started from the desktop may not have on its `PATH`. The one
 *  configured may be the file or the directory it is in, or the one of a
 *  download with its `bin`, from `~`. */
export function findTondo(configured: string, env: NodeJS.ProcessEnv = process.env, home = os.homedir()): string | undefined {
  if (configured.trim()) {
    return places(configured, home).find(isExecutable);
  }
  const dirs = (env.PATH ?? '').split(path.delimiter).filter((dir) => dir !== '');
  dirs.push(path.join(home, '.cargo', 'bin'), path.join(home, '.local', 'bin'));
  return dirs.map((dir) => path.join(dir, NAME)).find(isExecutable);
}

/** Where the setting may mean `tondo` is: as written without quotes, with
 *  `~` as the home, then in it as a directory, then in its `bin`. */
function places(configured: string, home: string): string[] {
  const given = configured.trim().replace(/^(["'])(.*)\1$/, '$2').replace(/^~(?=$|[\\/])/, home);
  return [given, path.join(given, NAME), path.join(given, 'bin', NAME)];
}

/** Why `tondo.path` gives no `tondo`, and what to write instead. */
export function misconfigured(configured: string, home = os.homedir()): string {
  const file = places(configured, home).find((f) => fs.statSync(f, { throwIfNoEntry: false })?.isFile());
  if (file) {
    return `\`tondo.path\` gives ${file}, which may not be run: make it executable with \`chmod +x ${file}\`.`;
  }
  return `\`tondo.path\` is "${configured}", where there is no tondo: set it to the executable, as /Users/me/tondo/bin/tondo, or to the directory it is in, or leave it empty to use the one on PATH.`;
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
