# Una app de escritorio con ventana propia (`std/ui`)

La evolución de [`examples/web/desktop`](../desktop/): la misma app (webserver embebido en un
puerto libre del SO + UI en HTML servida por el propio binario), pero en una **ventana nativa
con el webview del sistema** en vez del navegador. El IPC JS↔raylang sigue siendo el framework
web — `fetch` contra tus handlers.

```sh
cd examples/web/desktop_window
ray run                # se abre la ventana; cerrarla termina el programa
```

Y como binario único de código máquina:

```sh
ray build --native main.ray -o desktop-window-demo
./desktop-window-demo
```

Las piezas nuevas sobre el patrón de la fase 0:

1. **`ui.open(title, url, w, h)`** en cuanto el servidor acepta (mismo sondeo del puerto) — no
   hay `ui.run()`: el runtime captura el hilo principal por su cuenta.
2. **El ciclo de vida es un evento**: una fibra espera `ui.next_event()` (aparca, sin sondeo) y
   con `closed` de la ventana hace `exit(0)` — el botón rojo Y el botón "Salir" de la página
   convergen ahí.
3. El resto (puerto libre, handlers, `/quit`) es idéntico a la fase 0.

`std/ui` corre real en macOS; con `RAY_UI_BACKEND=headless` (tests/CI) las ventanas son filas en
memoria. El arco completo está clasificado en `IDEAS.md` §80.
