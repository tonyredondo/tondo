// What each project is made of, as `tondo graph` says: a view of its
// packages, modules, files and imports, and the graph in Mermaid.

import * as path from 'node:path';
import * as vscode from 'vscode';
import { exec, placeOf } from './cli';
import { Graphed, graphed } from './report';

/** A node of the view: a project, a package, a module, a file, or an
 *  import, which opens the line that imports. */
type Node =
  | { kind: 'project'; dir: string }
  | { kind: 'package'; dir: string; package: Graphed }
  | { kind: 'module'; module: Graphed['modules'][number] }
  | { kind: 'file'; file: string }
  | { kind: 'import'; module: string; file: string; line: number };

class Modules implements vscode.TreeDataProvider<Node> {
  readonly changed = new vscode.EventEmitter<void>();
  readonly onDidChangeTreeData = this.changed.event;
  private graphs = new Map<string, Promise<Graphed[]>>();

  refresh(): void {
    this.graphs.clear();
    this.changed.fire();
  }

  private graph(dir: string): Promise<Graphed[]> {
    let graph = this.graphs.get(dir);
    if (!graph) {
      graph = exec(['graph', dir, '--json'], dir).then((ran) => graphed(ran.stdout));
      this.graphs.set(dir, graph);
    }
    return graph;
  }

  getTreeItem(node: Node): vscode.TreeItem {
    const collapsed = vscode.TreeItemCollapsibleState.Collapsed;
    switch (node.kind) {
      case 'project':
        return Object.assign(new vscode.TreeItem(path.basename(node.dir), vscode.TreeItemCollapsibleState.Expanded), {
          iconPath: new vscode.ThemeIcon('project'),
          description: node.dir,
          contextValue: 'project',
        });
      case 'package':
        return Object.assign(new vscode.TreeItem(node.package.name, collapsed), {
          iconPath: new vscode.ThemeIcon('package'),
          description: node.package.dir,
        });
      case 'module':
        return Object.assign(new vscode.TreeItem(node.module.name, node.module.files.length || node.module.imports.length ? collapsed : 0), {
          iconPath: new vscode.ThemeIcon('symbol-module'),
        });
      case 'file':
        return Object.assign(new vscode.TreeItem(vscode.Uri.file(node.file)), {
          command: { command: 'vscode.open', title: 'Open', arguments: [vscode.Uri.file(node.file)] },
        });
      case 'import': {
        const at = new vscode.Range(node.line - 1, 0, node.line - 1, 0);
        return Object.assign(new vscode.TreeItem(`imports ${node.module}`), {
          iconPath: new vscode.ThemeIcon('arrow-right'),
          command: { command: 'vscode.open', title: 'Open', arguments: [vscode.Uri.file(node.file), { selection: at }] },
        });
      }
    }
  }

  async getChildren(node?: Node): Promise<Node[]> {
    if (!node) {
      const found = await vscode.workspace.findFiles('**/{tondo.toml,src/main.to}', '**/{target,node_modules,.git}/**');
      const dirs = new Set(
        found.map((file) => (path.basename(file.fsPath) === 'main.to' ? path.dirname(path.dirname(file.fsPath)) : path.dirname(file.fsPath))),
      );
      return [...dirs].sort().map((dir) => ({ kind: 'project', dir }));
    }
    switch (node.kind) {
      case 'project':
        return (await this.graph(node.dir)).map((p) => ({ kind: 'package', dir: node.dir, package: p }));
      case 'package':
        return node.package.modules.map((module) => ({ kind: 'module', module }));
      case 'module':
        return [
          ...node.module.files.map((file): Node => ({ kind: 'file', file })),
          ...node.module.imports.map((i): Node => ({ kind: 'import', module: i.module, file: i.file, line: i.line })),
        ];
      default:
        return [];
    }
  }
}

export function registerModules(context: vscode.ExtensionContext): void {
  const modules = new Modules();
  let pending: NodeJS.Timeout | undefined;
  context.subscriptions.push(
    vscode.window.registerTreeDataProvider('tondoModules', modules),
    vscode.commands.registerCommand('tondo.refreshModules', () => modules.refresh()),
    vscode.commands.registerCommand('tondo.moduleGraph', moduleGraph),
    // What a file imports changes when it is saved.
    vscode.workspace.onDidSaveTextDocument((document) => {
      if (document.languageId === 'tondo') {
        clearTimeout(pending);
        pending = setTimeout(() => modules.refresh(), 1000);
      }
    }),
  );
}

/** The graph of the modules of the project of the file open, or chosen,
 *  in Mermaid, in a Markdown document beside it. */
async function moduleGraph(node?: Node): Promise<void> {
  const file = vscode.window.activeTextEditor?.document.uri;
  const at = node?.kind === 'project' ? node.dir : file?.scheme === 'file' ? file.fsPath : vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!at) {
    void vscode.window.showInformationMessage('Open a Tondo file to show the graph of its project.');
    return;
  }
  const where = await placeOf(at).catch(() => undefined);
  const dir = where?.project ? where.root : path.extname(at) ? path.dirname(at) : at;
  const ran = await exec(['graph', where?.project ? dir : at, '--mermaid'], dir);
  if (ran.code !== 0) {
    void vscode.window.showErrorMessage(ran.stderr.trim() || 'tondo graph failed');
    return;
  }
  const content = `# Modules of ${path.basename(dir)}\n\n\`\`\`mermaid\n${ran.stdout}\`\`\`\n`;
  const document = await vscode.workspace.openTextDocument({ content, language: 'markdown' });
  await vscode.window.showTextDocument(document, vscode.ViewColumn.Beside);
}
