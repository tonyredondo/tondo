// The examples of the workspace's Markdown documents, which `tondo
// doc-test` checks: in the Testing view, one item each, and their failures
// on the lines of the documents.

import * as path from 'node:path';
import * as vscode from 'vscode';
import { exec } from './cli';
import { examples, failures } from './report';

export class Documents {
  private readonly items = new Map<string, vscode.TestItem>();
  private readonly problems = vscode.languages.createDiagnosticCollection('tondo-docs');

  constructor(private readonly controller: vscode.TestController) {}

  /** The documents with examples, each with its examples. */
  async discover(): Promise<void> {
    const files = await vscode.workspace.findFiles('**/*.md', '**/{target,node_modules,.git}/**');
    await Promise.all(files.map((file) => this.read(file)));
  }

  async read(file: vscode.Uri): Promise<void> {
    const text = Buffer.from(await vscode.workspace.fs.readFile(file)).toString('utf8');
    const found = examples(text);
    const id = `doc:${file.fsPath}`;
    if (found.length === 0) {
      if (this.items.delete(id)) {
        this.controller.items.delete(id);
      }
      return;
    }
    const folder = vscode.workspace.getWorkspaceFolder(file)?.uri.fsPath ?? path.dirname(file.fsPath);
    const document = this.items.get(id) ?? this.controller.createTestItem(id, path.relative(folder, file.fsPath), file);
    this.items.set(id, document);
    this.controller.items.add(document);
    document.children.replace(
      found.map(({ line, header }) => {
        const example = this.controller.createTestItem(`${id}#${line + 1}`, `line ${line + 1}: ${header}`, file);
        example.range = new vscode.Range(line, 0, line, 0);
        return example;
      }),
    );
  }

  /** Whether an item is a document's or an example's. */
  owns(item: vscode.TestItem): boolean {
    return item.id.startsWith('doc:');
  }

  /** Every document. */
  all(): vscode.TestItem[] {
    return [...this.items.values()];
  }

  /** Checks the documents of the items, and shows how each example did. */
  async run(testRun: vscode.TestRun, items: vscode.TestItem[], token: vscode.CancellationToken): Promise<void> {
    const documents = new Set(items.map((item) => (item.parent ?? item).id));
    for (const id of documents) {
      const document = this.items.get(id);
      if (!document?.uri || token.isCancellationRequested) {
        continue;
      }
      const chosen: vscode.TestItem[] = [];
      document.children.forEach((example) => chosen.push(example));
      const wanted = items.some((item) => item.id === id) ? chosen : chosen.filter((example) => items.includes(example));
      wanted.forEach((example) => testRun.started(example));
      const ran = await exec(['doc-test', document.uri.fsPath], path.dirname(document.uri.fsPath), token);
      const failed = failures(ran.stderr);
      const problems: vscode.Diagnostic[] = [];
      for (const example of wanted) {
        const line = Number(example.id.split('#').pop());
        const message = failed.get(line);
        if (message === undefined) {
          testRun.passed(example);
          continue;
        }
        const failure = new vscode.TestMessage(message);
        failure.location = new vscode.Location(document.uri, new vscode.Position(line - 1, 0));
        testRun.failed(example, failure);
        problems.push(new vscode.Diagnostic(new vscode.Range(line - 1, 0, line - 1, 8), message.split('\n')[0], vscode.DiagnosticSeverity.Error));
      }
      this.problems.set(document.uri, problems);
    }
  }
}
