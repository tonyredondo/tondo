// What runs inside VS Code: every command of the extension is there, and those
// of the project do what they say, with what they ask answered here.

import * as assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import * as vscode from 'vscode';

const manifest = () => {
  const root = path.resolve(__dirname, '../..');
  return JSON.parse(fs.readFileSync(path.join(root, 'package.json'), 'utf8')) as {
    contributes: { commands: { command: string }[] };
  };
};

const folder = () => vscode.workspace.workspaceFolders![0].uri.fsPath;

/** What the window asks next is answered by these, in order. */
function answering(answers: string[]): () => void {
  const window = vscode.window as { showQuickPick: unknown; showInputBox: unknown };
  const { showQuickPick, showInputBox } = window;
  const next = async (items?: unknown) => {
    const answer = answers.shift();
    const offered = (await items) as { label: string }[] | undefined;
    return offered?.find?.((item) => item.label === answer) ?? answer;
  };
  window.showQuickPick = next;
  window.showInputBox = next;
  return () => Object.assign(window, { showQuickPick, showInputBox });
}

/** Runs a command, answering what it asks. */
async function execute(command: string, answers: string[] = []): Promise<void> {
  const restore = answering(answers);
  try {
    await vscode.commands.executeCommand(command);
  } finally {
    restore();
  }
}

/** What the document a command opened says. */
async function shown(command: string, answers: string[] = []): Promise<string> {
  const before = vscode.window.activeTextEditor?.document;
  await execute(command, answers);
  const opened = () => vscode.window.activeTextEditor?.document;
  const document = await until(() => (opened() !== before ? opened() : undefined), command);
  return document.getText();
}

async function until<T>(found: () => T | undefined, what: string, seconds = 60): Promise<T> {
  const end = Date.now() + seconds * 1000;
  for (;;) {
    const value = found();
    if (value) {
      return value;
    }
    if (Date.now() > end) {
      throw new Error(`nothing came of ${what}`);
    }
    await new Promise((done) => setTimeout(done, 100));
  }
}

/** The exit code of the task a command starts. */
async function ended(command: string, answers: string[] = []): Promise<number | undefined> {
  const code = new Promise<number | undefined>((done) => {
    const listening = vscode.tasks.onDidEndTaskProcess((event) => {
      listening.dispose();
      done(event.exitCode);
    });
  });
  await execute(command, answers);
  return code;
}

const openMain = async () => {
  const document = await vscode.workspace.openTextDocument(path.join(folder(), 'src/main.to'));
  await vscode.window.showTextDocument(document);
  return document;
};

const tests: [string, () => Promise<void>][] = [
  [
    'every command is registered',
    async () => {
      const extension = vscode.extensions.all.find((one) => one.packageJSON.name === 'tondo');
      await extension!.activate();
      const known = new Set(await vscode.commands.getCommands(true));
      const missing = manifest().contributes.commands.map(({ command }) => command).filter((command) => !known.has(command));
      assert.deepEqual(missing, []);
    },
  ],
  [
    'the extension says what the language looks like',
    async () => {
      const contributed = vscode.extensions.all.find((one) => one.packageJSON.name === 'tondo')!.packageJSON;
      const root = path.resolve(__dirname, '../..');
      const language = contributed.contributes.languages[0];
      for (const icon of [contributed.icon, language.icon.light, language.icon.dark]) {
        assert.ok(fs.existsSync(path.join(root, icon)), icon);
      }
    },
  ],
  [
    'the language server marks a problem',
    async () => {
      const document = await openMain();
      const editor = vscode.window.activeTextEditor!;
      await editor.edit((edit) => edit.insert(new vscode.Position(5, 4), 'let unused = 1\n    '));
      await until(() => vscode.languages.getDiagnostics(document.uri).find((one) => String(one.message).includes('unused')), 'a warning');
      await vscode.commands.executeCommand('workbench.action.revertAndCloseActiveEditor');
    },
  ],
  [
    'the guide and the API',
    async () => {
      assert.match(await shown('tondo.guide', ['runbooks']), /# Runbooks/);
      await openMain();
      assert.match(await shown('tondo.showApi', ['std.cli']), /pub fn markup/);
      await openMain();
      assert.match(await shown('tondo.showPrivateApi', ['std.cli']), /fn markup/);
      await openMain();
      assert.match(await shown('tondo.searchApi', ['table', 'table']), /pub fn table/);
    },
  ],
  [
    'what a project is made of',
    async () => {
      await openMain();
      assert.match(await shown('tondo.compact'), /console\.println/);
      await openMain();
      assert.match(await shown('tondo.exportGraph', ['dot']), /digraph/);
    },
  ],
  [
    'what nothing reaches is marked',
    async () => {
      const document = await openMain();
      await execute('tondo.unreached');
      await until(() => vscode.languages.getDiagnostics(document.uri).find((one) => one.message.includes('nobody')), 'a mark');
    },
  ],
  [
    'dependencies and caches, in a terminal',
    async () => {
      await openMain();
      fs.mkdirSync(path.join(folder(), 'lib/src'), { recursive: true });
      fs.writeFileSync(path.join(folder(), 'lib/tondo.toml'), '[package]\nname = "lib"\n');
      fs.writeFileSync(path.join(folder(), 'lib/src/lib.to'), 'pub fn one(): Int {\n    1\n}\n');
      assert.equal(await ended('tondo.addDependency', ['path', 'lib', 'lib']), 0);
      assert.match(fs.readFileSync(path.join(folder(), 'tondo.toml'), 'utf8'), /lib/);
      assert.equal(await ended('tondo.removeDependency', ['lib']), 0);
      assert.equal(await ended('tondo.updateDependencies'), 0);
      assert.equal(await ended('tondo.clean', ['The cache of builds']), 0);
    },
  ],
  [
    'a library for C, and bindings of a header',
    async () => {
      const dir = path.join(folder(), 'clib');
      fs.mkdirSync(path.join(dir, 'src'), { recursive: true });
      fs.writeFileSync(path.join(dir, 'tondo.toml'), '[package]\nname = "clib"\n');
      const code = 'import std.ffi\n\npub extern fn add_one(value: ffi.CInt): ffi.CInt {\n    value + 1\n}\n';
      fs.writeFileSync(path.join(dir, 'src/lib.to'), code);
      fs.writeFileSync(path.join(dir, 'demo.h'), 'int demo(int value);\n');
      const document = await vscode.workspace.openTextDocument(path.join(dir, 'src/lib.to'));
      await vscode.window.showTextDocument(document);
      assert.equal(await ended('tondo.buildLibrary'), 0);
      assert.equal(await ended('tondo.bind', ['demo.h --output src/demo.to']), 0);
      assert.match(fs.readFileSync(path.join(dir, 'src/demo.to'), 'utf8'), /extern fn demo/);
    },
  ],
  [
    'a folder becomes a project',
    async () => {
      const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'tondo-bare-'));
      fs.writeFileSync(path.join(dir, 'one.to'), 'fn main() {}\n');
      const document = await vscode.workspace.openTextDocument(path.join(dir, 'one.to'));
      await vscode.window.showTextDocument(document);
      assert.equal(await ended('tondo.initProject', ['bare']), 0);
      assert.ok(fs.existsSync(path.join(dir, 'tondo.toml')));
    },
  ],
  [
    'a program runs and is debugged',
    async () => {
      await openMain();
      assert.equal(await ended('tondo.run'), 0);
      const session = new Promise<void>((done) => {
        const listening = vscode.debug.onDidTerminateDebugSession(() => {
          listening.dispose();
          done();
        });
      });
      const program = path.join(folder(), 'src/main.to');
      const config = { type: 'tondo', name: 'run', request: 'launch', program };
      assert.equal(await vscode.debug.startDebugging(vscode.workspace.workspaceFolders![0], config), true);
      await session;
    },
  ],
  [
    'what a machine needs',
    async () => {
      await openMain();
      assert.equal(await ended('tondo.targetPack', [path.join(folder(), 'packs')]), 0);
      await execute('tondo.dumpPlugin', ['gdb']);
      assert.match(await vscode.env.clipboard.readText(), /^source /);
    },
  ],
  [
    'any command of the command line',
    async () => {
      await openMain();
      assert.equal(await ended('tondo.runCommand', ['version']), 0);
      assert.equal(await ended('tondo.runCommand', ['explain P0001']), 0);
    },
  ],
];

export async function run(): Promise<void> {
  const failed: string[] = [];
  for (const [name, test] of tests) {
    try {
      await test();
      console.log(`ok ${name}`);
    } catch (error) {
      console.log(`FAILED ${name}: ${String(error)}`);
      failed.push(name);
    }
  }
  if (failed.length > 0) {
    throw new Error(`${failed.length} failed: ${failed.join(', ')}`);
  }
}
