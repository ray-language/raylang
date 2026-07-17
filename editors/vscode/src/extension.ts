// Cliente del Language Server de raylang para VSCode (M10.2c).
//
// La extensión hasta 0.9.0 era *solo declarativa* (gramática TextMate): VSCode leía el
// package.json y coloreaba, sin ejecutar código. Para los diagnósticos en vivo hace falta
// ejecutar código que lance el servidor (`ray lsp`) y traduzca el protocolo LSP a la UI de
// VSCode. Eso es lo que hace este módulo, apoyándose en `vscode-languageclient` (la única
// dependencia npm, del lado del editor; el binario de raylang sigue sin dependencias).

import * as vscode from "vscode";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  TransportKind,
  State,
} from "vscode-languageclient/node";

let client: LanguageClient | undefined;

// Resuelve el binario del servidor. Si `configured` es una ruta (lleva separador), se usa tal
// cual. Si es un nombre pelado (`ray`), se deja que el PATH lo resuelva — PERO en macOS VSCode
// lanzado desde el Dock NO hereda `~/.local/bin` (donde lo pone el instalador), así que probamos
// primero las ubicaciones típicas de instalación y solo caemos al nombre pelado si no aparece.
function resolveServerPath(configured: string): string {
  if (configured.includes(path.sep) || configured.includes("/")) {
    return configured; // ruta explícita: respétala
  }
  const home = os.homedir();
  const candidatos = [
    path.join(home, ".local", "bin", configured), // instalador (install.sh)
    path.join("/usr", "local", "bin", configured), // Homebrew Intel / manual
    path.join("/opt", "homebrew", "bin", configured), // Homebrew Apple Silicon
    path.join(home, ".cargo", "bin", configured), // cargo install
  ];
  for (const c of candidatos) {
    try {
      if (fs.existsSync(c)) return c;
    } catch {
      /* ignora errores de fs y sigue probando */
    }
  }
  return configured; // no se encontró en sitios conocidos: que lo resuelva el PATH
}

// VSCode llama a `activate` al abrir el primer archivo .ray (ver activationEvents).
export function activate(context: vscode.ExtensionContext): void {
  const config = vscode.workspace.getConfiguration("raylang");
  if (!config.get<boolean>("enableLsp", true)) {
    // El usuario desactivó el LSP: la extensión se queda en solo-coloreado.
    return;
  }

  // Cómo arrancar el servidor: el binario `ray` con el subcomando `lsp`, hablando por stdio.
  // El binario de producto es `ray` (M39a); `raylang` es un alias que puede no estar instalado,
  // así que apuntar a él por defecto dejaba el LSP sin arrancar (solo quedaba el coloreado).
  const serverPath = resolveServerPath(config.get<string>("serverPath", "ray"));
  const server = {
    command: serverPath,
    args: ["lsp"],
    transport: TransportKind.stdio,
  };
  const serverOptions: ServerOptions = { run: server, debug: server };

  // A qué documentos se aplica: los del lenguaje 'raylang'.
  const clientOptions: LanguageClientOptions = {
    documentSelector: [
      { scheme: "file", language: "raylang" },
      // M55: los templates compilados (.ray.html) reciben diagnósticos del mismo servidor
      // (errores del template + errores de tipos del módulo generado, mapeados a sus líneas).
      // Desde v0.17 los .ray.html abren como lenguaje `html` (IntelliSense nativo de HTML/CSS/JS
      // de VSCode) y el servidor raylang se conecta por PATRÓN, solo a los .ray.html — el server
      // distingue templates por el sufijo del URI, no por el languageId.
      { scheme: "file", language: "html", pattern: "**/*.ray.html" },
    ],
  };

  client = new LanguageClient(
    "raylang",
    "raylang Language Server",
    serverOptions,
    clientOptions,
  );

  // Si el servidor no llega a arrancar (típicamente: `ray` no está en el PATH que ve VSCode,
  // p. ej. lanzado desde el Dock en macOS, que no hereda `~/.local/bin`), avisamos con un
  // mensaje accionable en vez de fallar en SILENCIO (que es lo que dejaba "solo coloreado").
  client.onDidChangeState((e) => {
    if (e.newState === State.Stopped) {
      vscode.window.showErrorMessage(
        `raylang: no se pudo arrancar el Language Server ('${serverPath} lsp'). ` +
          `Comprueba que 'ray' esté en el PATH de VSCode, o fija 'raylang.serverPath' ` +
          `a la ruta absoluta del binario (p. ej. ~/.local/bin/ray).`,
      );
    }
  });

  // `start()` lanza el servidor y registra el cliente para que se cierre con la extensión.
  // Un fallo al spawnear el proceso rechaza la promesa: lo capturamos para no dejar una
  // "unhandled rejection" (el aviso ya lo da el handler de estado de arriba).
  client.start().catch(() => {
    /* el mensaje se muestra en onDidChangeState(Stopped) */
  });
  context.subscriptions.push(client);
}

// Al desactivarse la extensión, se detiene el servidor ordenadamente.
export function deactivate(): Thenable<void> | undefined {
  return client?.stop();
}
