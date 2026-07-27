# Banco de carga web

Mide **throughput sostenible bajo un SLO de latencia**: no "req/s máximo". Es el tercer arnés
del repo y no se solapa con los otros dos:

| arnés | compara | eje |
|---|---|---|
| `benchmarks/` | intérprete vs VM, y VM vs baseline commiteado | gate de regresión |
| `benchmarks/poly/` | raylang vs 9 lenguajes, programas de CPU/memoria | latencia de proceso |
| **`benchmarks/web/`** | servidores HTTP de raylang vs Rust/Go/Node | **carga sostenida** |

```sh
./build-all.sh                          # compila ray (nativo), Go y hyper (node no compila)
./webbench.py                           # las cuatro implementaciones, escalera por defecto
./webbench.py --only ray,hyper          # solo esas
./webbench.py --rates 80000,100000,120000   # afina cuando ya sabes el vecindario
./webbench.py --export-md /tmp/web.md   # tabla + bloque de entorno
```

O con el Makefile: `make bench-web` / `make bench-web-build`.

## Escalones: pelado vs framework

La comparación solo significa algo entre capas equivalentes:

| escalón | raylang | referencias |
|---|---|---|
| **pelado** (este banco) | `net/webserver` | Rust hyper · Go `net/http` · `node:http` |
| **framework** (pendiente) | `web/framework` | express/fastify · chi/gin |

Comparar `web/framework` contra hyper mezclaría dos costes que las investigaciones ya
demostraron que se atribuyen por separado
([`docs/investigacion-overhead-framework-express.md`](../../docs/investigacion-overhead-framework-express.md)
§3: la maquinaria del framework son ~72 µs, las rutas y el logging otros ~41 µs). El escalón
pelado es además el que decide si optimizar el router tiene sentido siquiera: si el techo de
I/O ya está lejos, el router no es el problema.

**hyper no es un rival, es el techo.** Dice cuánto da la máquina cuando el servidor es
prácticamente el syscall; cada resultado se lee como fracción de él.

## Método

1. **Levanta las cuatro a la vez**, cada una en su puerto, y espera a que **acepten**
   conexiones (no un sleep fijo: node tarda ~40 ms en levantar y un binario nativo ~3 ms;
   esperar el evento real hace la comparación honesta y falla rápido si algo no arranca).
2. **Verifica que responden lo mismo** (status + cuerpo exacto). Es el equivalente del
   checksum del banco poliglota: dos servidores que no sirven lo mismo no son comparables.
3. **Calentamiento** descartado por implementación.
4. **Escalones de tasa fija**, intercalados con rotación (ver abajo): `-q 5k, 10k, 20k…`
   Para cada escalón se registra p50/p99/p99.9 y la tasa **realmente conseguida**.
5. **Veredicto** = la tasa más alta que cumple el SLO (default p99 ≤ 10 ms) **y** sostiene
   ≥99 % de lo pedido. Un servidor que no llega a la tasa tocó su techo de throughput; su p99
   en ese escalón (segundos, por encolamiento) describe el régimen de saturación, no una cola
   patológica — el arnés distingue los dos fallos en la columna "primer escalón fallido".
6. Cuando una implementación no sostiene, **se le corta la escalera**: los escalones por
   encima solo repiten el mismo régimen.

### `oha` siempre con `-q` y `--latency-correction`

Cableado en el arnés, no opcional. Sin `-q`, oha es **closed-loop**: mantiene N conexiones y
espera la respuesta antes de mandar la siguiente, así que cuando el servidor se atasca el
generador también deja de mandar y el stall **nunca se registra como latencia**. Los p99 salen
bonitos justo cuando el sistema está peor. Es *coordinated omission*, y es el error más fácil
de cometer y más difícil de detectar leyendo el resultado.

Por eso el eje del experimento es la **tasa de llegada**, no la concurrencia: `-c` se fija y se
barre `-q` hasta que la p99 hace rodilla.

### Rondas intercaladas con rotación

En cada escalón se miden **todas** las implementaciones vivas, y el orden rota
(`A B C / B C A / C A B …`) — la misma disciplina que `poly/benchlib.run_variants`. El drift
ambiental (térmico, procesos de fondo) se reparte entre todas en vez de caer entero sobre la
que tocaba, y la rotación cancela el sesgo de posición.

No es una precaución teórica:
[`docs/investigacion-p99-framework-web.md`](../../docs/investigacion-p99-framework-web.md) §12
documenta una sesión donde, con corridas consecutivas, **el orden determinó el resultado por
completo** — invertir el orden invirtió el signo de la conclusión. Por lo mismo los servidores
se levantan **una vez** y viven toda la sesión (solo uno recibe carga a la vez; los demás
consumen ~0 CPU): arrancar y parar entre escalones reintroduce exactamente el sesgo de §12
(TIME_WAIT y limpieza del kernel con gaps cortos).

## Loopback

⚠️ **Las cifras de este banco medidas en loopback no son publicables.** Con el generador en la
misma máquina, `oha` compite por los mismos cores que el servidor: el techo que ves es en parte
la capacidad total de la máquina repartida entre los dos procesos, no la del servidor. Sirve
para depurar el arnés y para **comparaciones relativas** entre implementaciones medidas en la
misma sesión. El `--export-md` estampa el origen del generador en el bloque de entorno, para que
ningún export se lea fuera de contexto.

## Generador en otra máquina

```sh
./webbench.py --bind 10.0.0.10 --generator-host roberto@10.0.0.20

# usuario y llave por separado, si prefieres no mezclarlos en el host:
./webbench.py --bind 10.0.0.10 --generator-host 10.0.0.20 \
              --ssh-user roberto -i ~/.ssh/id_bench
```

`--bind` es la IP del enlace en la máquina servidor (los cuatro servidores la reciben como
argumento); `--generator-host` es el destino SSH donde corre `oha`. El readiness y la
verificación de respuesta siguen siendo locales —son chequeos de corrección, no de rendimiento—
y el intercalado con rotación se conserva sin coordinar dos máquinas.

**Usuario y llave.** `--generator-host` acepta el formato de `ssh` (`[usuario@]host`), y
`--ssh-user` / `--ssh-key` (`-i`) son la alternativa para darlos por separado; si el host ya
trae `usuario@`, ese gana. Sin `--ssh-key` se usa tu configuración normal (`~/.ssh/config`,
agente). Con ella se añade `IdentitiesOnly=yes`, porque si no `ssh` ofrece antes las claves del
agente y la `-i` explícita podría no llegar a usarse: el flag diría una cosa y la conexión haría
otra. Todo el SSH va con `BatchMode=yes`, para que una llave con passphrase falle en el acto en
vez de bloquear una sesión de medida esperando en un prompt.

### Requisitos, en orden

1. **El enlace.** Thunderbolt Bridge en ambos Macs con IPs manuales. Verifica que la ruta va por
   ahí y **mídelo**, no lo supongas:

   ```sh
   route get 10.0.0.20     # debe salir por bridge0, NO por en0/Wi-Fi
   iperf3 -s               # en el servidor; iperf3 -c 10.0.0.10 en el generador
   ```

   Medido el 27 jul 2026 en el enlace M3↔M4: **5.35 Gbit/s** sostenidos, con bastante jitter
   entre segundos (4.58–7.44). Para esta carga el ancho de banda **no es el límite**: a 160k rps
   una petición+respuesta con framing son ~385 B, o sea ~0.5 Gbit/s — un 9 % del enlace. Lo que
   hay que vigilar es la **tasa de paquetes** (~320k pps a 160k rps), y ahí `iperf3` con tramas
   de 1500 B no demuestra gran cosa: por eso el requisito 3 no es opcional.

2. **SSH sin passphrase y `oha` instalado en el generador.** El arnés hace un preflight
   (`command -v oha` por SSH) y aborta con un mensaje claro si falla — sin él, un SSH mal
   configurado se manifestaría como "ninguna implementación sostiene ninguna tasa", que es un
   diagnóstico pésimo.

   ```sh
   ssh-keygen -t ed25519 -N "" -f ~/.ssh/id_bench     # sin passphrase: BatchMode la rechaza
   ssh-copy-id -i ~/.ssh/id_bench roberto@10.0.0.20   # acepta la clave del host la 1ª vez
   ssh -i ~/.ssh/id_bench roberto@10.0.0.20 'brew install oha'
   ```

3. **Comprobar que el generador NO es el nuevo cuello.** Apunta el `oha` remoto contra **hyper**
   (el techo) y verifica que supera lo que viste en loopback (~163k rps). Si la máquina
   generadora no puede producir más de lo que el servidor sirve, has movido el cuello de sitio y
   sigues midiendo el generador — con el agravante de que ya no lo sospechas:

   ```sh
   ./webbench.py --bind 10.0.0.10 --generator-host 10.0.0.20 --only hyper \
                 --rates 160000,200000,260000
   ```

4. **Límites del SO.** El arnés sube el suyo y los servidores lo heredan (ver abajo); en el
   generador lo sube la propia shell remota antes de lanzar `oha`. Vigila además los puertos
   efímeros del generador (`sysctl net.inet.ip.portrange.*`) y `kern.ipc.somaxconn` en el
   servidor. macOS pedirá autorizar conexiones entrantes la primera vez.

### El sesgo de los descriptores de archivo

El arnés **sube el límite blando de fds y los servidores lo heredan**. No es cosmético, corrige
una desigualdad real: **raylang sube su propio límite blando al duro al arrancar**
(`src/lib.rs`), y el runtime de Go también (1.19+), pero **hyper y node no**. Lanzado desde una
shell con el default de macOS (`ulimit -n 256`), ray y Go correrían con ~138 000 fds y hyper/node
con 256 — una desventaja invisible en el resultado que crece con la concurrencia. Igualándolo en
el arnés, el veredicto deja de depender de la terminal desde la que se lanzó.

`--bind 127.0.0.1` junto con `--generator-host` es un error explícito: el generador remoto no
puede alcanzar loopback.

## Primer resultado (loopback, M3 Pro, 27 jul 2026)

Escalera por defecto, `-c 100`, 8 s por escalón, SLO p99 ≤ 10 ms. **Relativo, no citable**:

| implementación | tasa sostenida bajo SLO | techo observado |
|---|---|---|
| hyper | 120 000 rps | ~163 000 |
| **raylang** (`net/webserver`, nativo) | **80 000 rps** (0.67× hyper) | ~113 000 |
| Go `net/http` | 80 000 rps (0.67× hyper) | ~119 000 |
| `node:http` | 40 000 rps (0.33× hyper) | ~86 000 |

Lectura: en el escalón pelado, raylang está **en el mismo peldaño que Go `net/http`** (mismo
veredicto, techos a 113k vs 119k) y a ~0.67× del techo de I/O de la máquina. Node queda un
escalón por debajo, con la p99 disparándose ya a 80k. El resultado se repitió en dos sesiones
(una sin intercalado y otra con él) con los mismos veredictos.

## Pendiente

- **Correr con el generador remoto** — el arnés ya lo soporta (`--bind` + `--generator-host`) y
  el enlace está medido; falta el acceso SSH con `oha` en la máquina generadora, y después el
  requisito 3 de arriba. Es el prerrequisito de cualquier cifra publicable.
- **Escalón de framework**: `web/framework` vs express/fastify vs chi/gin.
- **Carga `json`**: la segunda categoría de TechEmpower, que engancha con `jsonserialize` del
  banco poliglota (coste de CPU por serializado ↔ req/s sostenidos).
- **Repeticiones por escalón**: hoy es una corrida por escalón. La rotación reparte el drift,
  pero no da barras de error; para un veredicto ajustado (dos implementaciones a un escalón de
  distancia) haría falta repetir y comparar dispersión, como hace `poly/` con mediana+MAD.
- **Linux/epoll**: todo esto es macOS/kqueue. `src/poll.rs` soporta epoll, pero los números no
  transfieren.
