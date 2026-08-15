import * as path from 'path';
import { ExtensionContext, workspace, commands, window } from 'vscode';
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
} from 'vscode-languageclient/node';

let client: LanguageClient;

export function activate(context: ExtensionContext) {
  const command = workspace.getConfiguration('c-script').get<string>('compilerPath') || 'linkc';

  const serverOptions: ServerOptions = {
    run: { command, args: ['lsp'] },
    debug: { command, args: ['lsp'] },
  };

  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ scheme: 'file', language: 'c-script' }],
    synchronize: {
      fileEvents: workspace.createFileSystemWatcher('**/*.link'),
    },
  };

  client = new LanguageClient(
    'cScriptLsp',
    'c-script Language Server',
    serverOptions,
    clientOptions
  );

  client.start();

  context.subscriptions.push(
    commands.registerCommand('c-script.runTests', () => {
      const activeDoc = window.activeTextEditor?.document;
      if (!activeDoc || !activeDoc.fileName.endsWith('.link')) {
        window.showWarningMessage('Abre un archivo .link para ejecutar sus pruebas.');
        return;
      }
      const terminal = window.createTerminal('Link Tests');
      terminal.show();
      terminal.sendText(`${command} test "${activeDoc.fileName}"`);
    }),
    commands.registerCommand('c-script.build', () => {
      const activeDoc = window.activeTextEditor?.document;
      if (!activeDoc || !activeDoc.fileName.endsWith('.link')) {
        window.showWarningMessage('Abre un archivo .link para generar el contrato.');
        return;
      }
      const terminal = window.createTerminal('Link Build');
      terminal.show();
      terminal.sendText(`${command} build "${activeDoc.fileName}" ./gen`);
    })
  );
}

export function deactivate(): Thenable<void> | undefined {
  if (!client) {
    return undefined;
  }
  return client.stop();
}
