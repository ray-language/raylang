# Una app de escritorio sin ventana propia (fase 0 del "Tauri de raylang")

El patrón más corto hacia una app de escritorio con raylang, **sin código nuevo del
lenguaje**: el binario levanta el framework web en `127.0.0.1` con un puerto libre que asigna
el SO y abre el navegador por defecto. La "ventana" es una pestaña; el "IPC" JS↔raylang es
`fetch` contra los handlers del framework — el framework ES el puente.

```sh
cd examples/web/desktop
ray run                # se abre el navegador solo
```

Y como binario único de código máquina:

```sh
ray build --native main.ray -o desktop-demo
./desktop-demo
```

Las tres piezas del patrón, todas superficie existente:

1. **Puerto libre**: `net.tcp_listen("127.0.0.1", 0)` deja que el SO elija; `net.local_port`
   lo lee y se libera para el webserver. Dos instancias de la app nunca chocan.
2. **Arranque sin carreras**: una fibra sondea el puerto (`tcp_connect_timeout` + cerrar)
   y abre el navegador (`open` / `xdg-open`) solo cuando el servidor ya acepta.
3. **Salir desde la UI**: un `POST /quit` responde y termina el proceso con `exit(0)` desde
   una fibra aparte (la respuesta llega al navegador antes de morir).

La evolución de este patrón hacia ventana propia (webview nativo), assets embebidos y
bundling está clasificada en `IDEAS.md` §80.
