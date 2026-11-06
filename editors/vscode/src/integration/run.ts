// The extension in a real VS Code: `npm run test:editor`, with TONDO_BIN naming
// the `tondo` to use, on a project made for the run.

import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { runTests } from '@vscode/test-electron';

const MAIN = `import std.console

fn nobody() {}

fn main() {
    console.println("hello")
}
`;

async function main(): Promise<void> {
  const tondo = process.env.TONDO_BIN;
  if (!tondo) {
    throw new Error('TONDO_BIN names the `tondo` to test with');
  }
  const project = fs.mkdtempSync(path.join(os.tmpdir(), 'tondo-editor-'));
  fs.mkdirSync(path.join(project, 'src'));
  fs.mkdirSync(path.join(project, '.vscode'));
  fs.writeFileSync(path.join(project, 'tondo.toml'), '[package]\nname = "app"\n');
  fs.writeFileSync(path.join(project, 'src/main.to'), MAIN);
  fs.writeFileSync(path.join(project, '.vscode/settings.json'), JSON.stringify({ 'tondo.path': tondo }));
  await runTests({
    extensionDevelopmentPath: path.resolve(__dirname, '../..'),
    extensionTestsPath: path.resolve(__dirname, 'suite.js'),
    launchArgs: [project, '--disable-gpu', '--disable-workspace-trust', '--skip-welcome'],
  });
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
