// Cliente del Language Server de raylang para VSCode (M10.2c).
//
// La extensión hasta 0.9.0 era *solo declarativa* (gramática TextMate): VSCode leía el
// package.json y coloreaba, sin ejecutar código. Para los diagnósticos en vivo hace falta
// ejecutar código que lance el servidor (`raylang --lsp`) y traduzca el protocolo LSP a la UI
// de VSCode. Eso es lo que hace este módulo, apoyándose en `vscode-languageclient` (la única
// dependencia npm, del lado del editor; el binario de raylang sigue sin dependencias).

import * as vscode from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  TransportKind,
} from "vscode-languageclient/node";

let client: LanguageClient | undefined;

// VSCode llama a `activate` al abrir el primer archivo .ray (ver activationEvents).
export function activate(context: vscode.ExtensionContext): void {
  const config = vscode.workspace.getConfiguration("raylang");
  if (!config.get<boolean>("enableLsp", true)) {
    // El usuario desactivó el LSP: la extensión se queda en solo-coloreado.
    return;
  }

  // Cómo arrancar el servidor: el binario de raylang con '--lsp', hablando por stdio.
  const serverPath = config.get<string>("serverPath", "raylang");
  const server = {
    command: serverPath,
    args: ["--lsp"],
    transport: TransportKind.stdio,
  };
  const serverOptions: ServerOptions = { run: server, debug: server };

  // A qué documentos se aplica: los del lenguaje 'raylang'.
  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ scheme: "file", language: "raylang" }],
  };

  client = new LanguageClient(
    "raylang",
    "raylang Language Server",
    serverOptions,
    clientOptions,
  );

  // `start()` lanza el servidor y registra el cliente para que se cierre con la extensión.
  client.start();
  context.subscriptions.push(client);
}

// Al desactivarse la extensión, se detiene el servidor ordenadamente.
export function deactivate(): Thenable<void> | undefined {
  return client?.stop();
}
