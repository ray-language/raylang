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
./webbench.py                           # escalón pelado, las cuatro, escalera por defecto
./webbench.py -w json                   # escalón de framework
./webbench.py --only ray,hyper          # solo esas
./webbench.py --rates 80000,100000,120000   # afina cuando ya sabes el vecindario
./webbench.py --export-md /tmp/web.md   # tabla + bloque de entorno
```

O con el Makefile: `make bench-web` / `make bench-web-build`.

## Escalones: pelado vs framework

La comparación solo significa algo entre capas equivalentes:

| escalón | qué se pide | raylang | referencias |
|---|---|---|---|
| **`plaintext`** (pelado) | `GET /` → `Hello, World!` | `net/webserver` | Rust hyper · Go `net/http` · `node:http` |
| **`json`** (framework) | `GET /users/42` → `{"id":"42","name":"Ada"}` | `web/framework` | Rust axum · Go chi · express |

Se eligen con `--workload` (`-w`); el default es `plaintext`.

El escalón de framework **no** sirve un texto fijo a propósito: un plaintext no ejercita ni el
router ni el serializador, que son justo la capa bajo examen. El endpoint lleva un parámetro de
ruta y devuelve JSON, y las cuatro implementaciones registran las **mismas 10 rutas** — el coste
de emparejar depende del tamaño de la tabla, así que igualarlo es parte de que la comparación
signifique algo. El `id` se interpola como string sin parsear en las cuatro, para que ninguna
pague un trabajo que otra no hace.

Comparar `web/framework` contra hyper mezclaría dos costes que las investigaciones ya
demostraron que se atribuyen por separado
([`docs/investigacion-overhead-framework-express.md`](../../docs/investigacion-overhead-framework-express.md)
§3: la maquinaria del framework son ~72 µs, las rutas y el logging otros ~41 µs). El escalón
pelado es además el que decide si optimizar el router tiene sentido siquiera: si el techo de
I/O ya está lejos, el router no es el problema.

**hyper y axum no son rivales, son el techo** de su escalón. Dicen cuánto da la máquina cuando el
servidor es prácticamente el syscall (más un match de rutas, en el caso de axum); cada resultado se
lee como fracción de ellos.

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

### `oha` siempre con `-q`

Cableado en el arnés, no opcional. Sin `-q`, oha es **closed-loop**: mantiene N conexiones y
espera la respuesta antes de mandar la siguiente, así que cuando el servidor se atasca el
generador también deja de mandar y el stall **nunca se registra como latencia**. Los p99 salen
bonitos justo cuando el sistema está peor. Es *coordinated omission*, y es el error más fácil
de cometer y más difícil de detectar leyendo el resultado.

Por eso el eje del experimento es la **tasa de llegada**, no la concurrencia: `-c` se fija y se
barre `-q` hasta que la p99 hace rodilla.

### La corrección de latencia es OPT-IN, y es una lección medida

`--latency-correction` empezó cableado en el arnés y **se retiró por medición**. Contra hyper a
5 000 rps con el generador remoto, cambiando solo ese flag:

| config | p50 | p99 | p99.9 |
|---|---|---|---|
| `-c 100` con corrección | 0.58 ms | 12.90 ms | 78.94 ms |
| `-c 100` sin corrección | 0.52 ms | 3.19 ms | 81.73 ms |
| `-c 10` con corrección | 0.52 ms | 25.06 ms | 86.05 ms |
| `-c 10` sin corrección | 0.46 ms | **2.17 ms** | **5.07 ms** |

Mismo rps y mismo p50; la cola se derrumba 10-17× al quitar el flag. **La cola la fabricaba la
corrección, no los servidores.** El razonamiento original estaba incompleto: la corrección mide
el retraso respecto al instante en que la petición *debía* haberse enviado, y eso arregla
coordinated omission en runs **closed-loop**. Con `-q` el run ya es open-loop, así que la
corrección no aporta y sí cobra como latencia del servidor los tropiezos de planificación del
propio `oha` (más visibles con el generador en otra máquina y con 100 conexiones despertando cada
20 ms).

Lo que protege de verdad contra coordinated omission en este arnés es el chequeo de **"¿sostuvo
la tasa pedida?"** (≥99 %): si el servidor no da abasto, la tasa conseguida se desploma y el
escalón se marca como techo. Ese guardián no se contamina con el jitter del generador.

`--correction` sigue disponible para comparar, y el bloque de entorno del `--export-md` registra
si estaba activa.

### Repeticiones, mediana y MAD

`--reps` (default **3**) mide cada escalón varias veces y reporta la **mediana** de cada métrica
más el **MAD** de la p99 (y su min-max). Mediana y MAD en vez de media y σ, por lo mismo que en
`poly/`: el ruido aquí es aditivo —una pausa del scheduler o un tropiezo del generador solo puede
sumar—, así que un outlier arrastra la media pero no la mediana.

El veredicto (¿sostiene? ¿dentro del SLO?) se decide sobre las **medianas**, no sobre una corrida
suelta: una repetición desafortunada no debe cortar la escalera ni tumbar un escalón.

Y el MAD no es decoración: es lo que permite **no** afirmar una jerarquía que los datos no
sostienen. Dos implementaciones comparten puesto (marcado con `=`) si sostienen la misma tasa y
sus ventanas de p99 (mediana ± 2·MAD) se solapan. Esto entró precisamente porque con una sola
corrida raylang y Go quedaban a un ~7 % de p99 — una diferencia que no se podía defender.

Las repeticiones se **intercalan igual que los escalones**: la repetición *r* de cada
implementación se mide junto a la *r* de las demás, y la rotación avanza en cada pasada. Así
ninguna acumula sus repeticiones en la misma ventana térmica.

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

## Resultado con generador remoto (27 jul 2026)

Servidor M3 Pro (11 cores) ↔ generador M4 (12 cores) por Thunderbolt bridge; `-c 100`, 8 s ×
**5 repeticiones** por escalón, SLO p99 ≤ 10 ms, sin corrección de latencia. Medianas, con el MAD
de la p99:

| implementación | tasa sostenida bajo SLO | p50 | p99 | p99 MAD | p99.9 | techo observado |
|---|---|---|---|---|---|---|
| hyper | **≥200 000 rps** (tope del generador) | 0.46 ms | 0.69 ms | ±0.00 | 1.11 ms | no alcanzado |
| **raylang** (`net/webserver`, nativo) | **160 000 rps** | **0.54 ms** | **0.80 ms** | ±0.03 | **1.68 ms** | ~165 600 |
| Go `net/http` | 120 000 rps | 0.80 ms | 2.07 ms | ±0.00 | 2.64 ms | ~120 700 |
| `node:http` | 40 000 rps | 0.86 ms | 2.48 ms | ±0.15 | 6.57 ms | ~61 800 |

**raylang sostiene 1.33× el throughput de Go** y le gana en las cuatro métricas. Comparados en el
mismo escalón (120k rps), donde los dos aguantan: p50 0.54 vs 0.80, p99 1.15 vs 2.07 (ventanas de
mediana ± 2·MAD disjuntas por mucho: `[1.13, 1.17]` contra `[2.06, 2.08]`), p99.9 1.88 vs 2.64.

Estas cifras son **posteriores a M97.2** (`try_call`), que quitó el `spawn` por petición del
webserver. Antes de ese cambio raylang empataba con Go en el veredicto (120k) y perdía la cola
profunda por 2.5× (6.59 vs 2.63 ms). La crónica completa —cómo se localizó el mecanismo y qué se
midió— está en
[`docs/investigacion-p999-webserver-nativo.md`](../../docs/investigacion-p999-webserver-nativo.md):

| raylang | antes de M97.2 | después |
|---|---|---|
| hilos de SO (100 conexiones) | 198 | 97 |
| p50 @120k | 0.65 ms | 0.54 ms |
| p99 @120k | 1.86 ms | 1.15 ms |
| p99.9 @120k | 6.59 ms | 1.88 ms |
| techo | ~129 500 rps | ~165 600 rps |

### Escalón de framework (`-w json`, 27 jul 2026)

Mismo enlace y método, `-c 100`, 8 s × 3 repeticiones, SLO p99 ≤ 10 ms:

| implementación | tasa sostenida bajo SLO | p50 | p99 | p99 MAD | p99.9 | techo observado |
|---|---|---|---|---|---|---|
| axum | **≥200 000 rps** (tope del generador) | 0.47 ms | 0.71 ms | ±0.01 | 1.39 ms | no alcanzado |
| **raylang** (`web/framework`) | **160 000 rps** | 0.56 ms | 0.81 ms | ±0.05 | 1.94 ms | ~165 500 |
| Go chi | 120 000 rps | 0.73 ms | 2.18 ms | ±0.00 | 2.72 ms | ~122 800 |
| express | 40 000 rps | 0.85 ms | 2.21 ms | ±0.03 | 4.25 ms | ~48 800 |

**`web/framework` sostiene 1.33× lo de Go+chi** — el mismo múltiplo que en el escalón pelado — y
express queda cuatro veces por debajo.

#### El impuesto de framework, por lenguaje

Cruzando cada framework con el servidor pelado del mismo lenguaje (los dos escalones se midieron
con el mismo enlace y método):

| | pelado | framework | impuesto |
|---|---|---|---|
| raylang | ~165 600 | ~165 500 | **~0 %** |
| Go | ~120 700 (`net/http`) | ~122 800 (chi) | ~0 % (dentro del ruido) |
| Node | ~61 800 (`node:http`) | ~48 800 (express) | **−21 %** |
| Rust | ≥200 000 (hyper) | ≥200 000 (axum) | no medible (tope del generador) |

chi es un router fino y no cuesta nada apreciable, como cabía esperar. express sí: se lleva un
quinto del techo de Node.

⚠️ **Cuidado con leer el ~0 % de raylang como "el framework es gratis".** El techo coincide con el
del servidor pelado hasta la tercera cifra significativa (165 600 vs 165 500), y eso no dice que la
capa de framework no cueste: dice que **no es ella la que limita**. El cuello está por debajo, en
`net/webserver` —el ítem del §6 de
[`docs/investigacion-p999-webserver-nativo.md`](../../docs/investigacion-p999-webserver-nativo.md):
un hilo de SO bloqueante por conexión—, y mientras ese techo no suba, el coste real del framework
seguirá enmascarado. La investigación de
[`investigacion-overhead-framework-express.md`](../../docs/investigacion-overhead-framework-express.md)
sí lo midió en aislamiento: ~7 µs por petición reconstruyendo la `App`, que a 165k rps son ~1.16
core-segundos por segundo (~10 % de los 11 cores) — real, pero no lo que topa.

### Dos límites de esta medición

**hyper no es un techo aquí, es un suelo.** Pasó los 200k del último escalón sin sudar (p99
0.69 ms) porque el que topa es el generador: midiendo la CPU de las dos máquinas durante una
corrida a 300k rps pedidos, el **generador estaba al 86 %** (28 % user + 58 % sys) mientras el
**servidor tenía 40 % idle**. Subir la escalera no sirve sin una segunda máquina generadora.

**El p99.9 a tasas bajas (5k-10k) es un artefacto**, no de los servidores: son ~90-104 ms en las
cuatro implementaciones a la vez —hyper incluida— y **desaparece al subir la carga** (≤13 ms desde
20k, ≤3 ms desde 80k). Un servidor no mejora su cola al cargarlo más. Con `-c 100` a 5k rps cada conexión pasa ~20 ms
ociosa, y el camino Thunderbolt-IP parece pagar el despertar. No afecta a la comparación (pega
igual a las cuatro) ni a los veredictos, que se deciden mucho más arriba. El enlace en reposo,
por contraste, es impecable: RTT 0.34/0.57/1.50 ms (min/avg/max) con stddev 0.12 en 500 pings.

## Pendiente

- ✅ **El p99.9 de raylang** (era 6.6 ms contra 2.6 de Go) fue el hallazgo más accionable de este
  banco y ya está **resuelto**: se perfiló con `sample` bajo carga, la causa era el `spawn` por
  petición del webserver, y M97.2 (`try_call`) lo bajó a 1.88 ms — mejor que Go. Crónica en
  `docs/investigacion-p999-webserver-nativo.md`.
- **Un techo nuevo para raylang**: ahora sostiene 160k y topa en ~165 600, más cerca del límite del
  generador (~200k) que del suyo propio. Con más margen de generación habría que re-medirlo.
- **Más repeticiones para node** — su veredicto se movió de 80k a 40k entre sesiones, mucho más
  que sus vecinos. Sin diagnosticar.
- **Un techo de verdad para hyper** — el generador topa antes que el servidor (~200k). Exige una
  segunda máquina generadora, o `oha` desde las dos a la vez sumando tasa.
- **Resolver el artefacto de tasas bajas** — si se quiere reportar p99.9 a 5-10k rps, hay que
  entender primero qué del camino Thunderbolt-IP añade esos ~95 ms con conexiones semi-ociosas.
- ✅ **Escalón de framework**: hecho (`-w json`, resultados arriba). Ampliable con fastify (la
  alternativa rápida de Node) y gin, si interesa cubrir más del ecosistema.
- **Subir el techo de `net/webserver`** — es ahora el límite de raylang en LOS DOS escalones
  (~165 500 en ambos), así que el coste real de `web/framework` no se puede medir hasta que suba.
  El sospechoso está identificado: un hilo de SO bloqueante por conexión (§6 de la investigación).
- **Carga `json`**: la segunda categoría de TechEmpower, que engancha con `jsonserialize` del
  banco poliglota (coste de CPU por serializado ↔ req/s sostenidos).
- **Repeticiones por escalón**: hoy es una corrida por escalón. La rotación reparte el drift,
  pero no da barras de error; para un veredicto ajustado (dos implementaciones a un escalón de
  distancia) haría falta repetir y comparar dispersión, como hace `poly/` con mediana+MAD.
- **Linux/epoll**: todo esto es macOS/kqueue. `src/poll.rs` soporta epoll, pero los números no
  transfieren.
