// The refactors of the language server that need an answer first: the
// server offers them, the person says how, and the server gives the edit
// once the program checks after it.

import * as vscode from 'vscode';

/** What the server answers: the edit, or why not. */
interface Answer {
  edit?: { changes: Record<string, { range: vscode.Range; newText: string }[]> };
  error?: string;
}

/** Applies what the server answered. */
async function applied(answer: Answer | undefined): Promise<void> {
  if (answer?.error) {
    void vscode.window.showErrorMessage(answer.error);
    return;
  }
  const edit = new vscode.WorkspaceEdit();
  for (const [uri, edits] of Object.entries(answer?.edit?.changes ?? {})) {
    for (const { range, newText } of edits) {
      const { start, end } = range;
      const at = new vscode.Range(start.line, start.character, end.line, end.character);
      edit.replace(vscode.Uri.parse(uri), at, newText);
    }
  }
  await vscode.workspace.applyEdit(edit);
}

export function registerRefactors(context: vscode.ExtensionContext): void {
  context.subscriptions.push(
    vscode.commands.registerCommand('tondo.changeSignature', async (uri: string, position: unknown, list: string) => {
      const prompt = 'The parameters: reorder, remove or add them; a new one says what its calls pass, as `limit: Int = 10`';
      const wanted = await vscode.window.showInputBox({ prompt, value: list });
      if (wanted !== undefined) {
        await applied(await vscode.commands.executeCommand<Answer>('tondo.signature', uri, position, wanted));
      }
    }),
    vscode.commands.registerCommand('tondo.groupParameters', async (uri: string, position: unknown, list: string) => {
      const prompt = `The type and the parameters it takes, of ${list}, as \`Point: x, y\``;
      const wanted = await vscode.window.showInputBox({ prompt });
      if (wanted !== undefined) {
        await applied(await vscode.commands.executeCommand<Answer>('tondo.group', uri, position, wanted));
      }
    }),
    vscode.commands.registerCommand('tondo.refactor', async (command: string, uri: string, position: unknown) => {
      await applied(await vscode.commands.executeCommand<Answer>(command, uri, position));
    }),
    vscode.commands.registerCommand('tondo.extractTrait', async (uri: string, position: unknown) => {
      const wanted = await vscode.window.showInputBox({ prompt: 'The trait and its methods, as `Shape: area, name`' });
      if (wanted !== undefined) {
        await applied(await vscode.commands.executeCommand<Answer>('tondo.trait', uri, position, wanted));
      }
    }),
    vscode.commands.registerCommand('tondo.moveToModule', async (uri: string, position: unknown) => {
      const modules = await vscode.commands.executeCommand<string[]>('tondo.modules', uri, position);
      const module = await vscode.window.showQuickPick(modules ?? [], { placeHolder: 'The module it goes to' });
      if (module) {
        await applied(await vscode.commands.executeCommand<Answer>('tondo.moveTo', uri, position, module));
      }
    }),
  );
}
