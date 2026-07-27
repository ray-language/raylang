#!/usr/bin/env python3
"""
webbench — banco de CARGA HTTP: mide throughput sostenible bajo un SLO de latencia.

No es el banco poliglota (`benchmarks/poly/`), que lanza un proceso y lo cronometra: aquí el
servidor vive toda la sesión y quien mide es un generador de carga externo (`oha`). El eje es
otro, así que el arnés es otro; lo único que se reusa de `poly/benchlib.py` es el bloque de
entorno y el formato de tabla/markdown.

Método (ver README.md §Método para el porqué de cada pieza):

  1. Levanta el servidor y espera a que ACEPTE conexiones (no un sleep fijo).
  2. Verifica que responde lo mismo que las demás implementaciones (cuerpo + status),
     con el mismo espíritu que el checksum del banco poliglota: si no sirven lo mismo,
     compararlos no significa nada.
  3. Calentamiento descartado.
  4. ESCALONES de tasa de llegada fija: -q 5k, 10k, 20k... Para cada uno registra p50/p99/
     p99.9 y la tasa REALMENTE conseguida.
  5. El veredicto es la tasa más alta que cumple el SLO (default: p99 <= 10 ms) Y sostiene
     al menos el 99% de la tasa pedida. NO se reporta "req/s máximo": el pico esconde
     justo la rodilla que interesa.
  6. Mata el servidor y espera a que el puerto se libere antes de la siguiente
     implementación.

`oha` se invoca SIEMPRE con `-q` (tasa fija, open-loop). Sin `-q`, oha es closed-loop: cuando
el servidor se atasca el generador deja de mandar y el stall nunca se registra — los p99 salen
bonitos justo cuando el sistema está peor (coordinated omission). Por eso la tasa no es un flag
opcional del arnés sino el eje del experimento.

`--latency-correction` en cambio es OPT-IN (`--correction`), y por medición: con `-q` el run ya
es open-loop, así que la corrección no aporta y sí cobra como latencia del servidor los
tropiezos de planificación del propio oha. Medido el 27 jul 2026 contra hyper a 5k rps con el
generador remoto: p50 idéntico y rps idéntico, pero p99 2.17→25.06 ms y p99.9 5.07→86.05 ms
solo por añadir el flag (ver README §La corrección de latencia). Lo que protege de verdad
contra coordinated omission aquí es el chequeo de "¿sostuvo la tasa pedida?", que no se
contamina con el jitter del generador.

Uso:
    ./webbench.py                       Mide todas las implementaciones de `plaintext`
    ./webbench.py --only ray,hyper      Solo esas
    ./webbench.py --rates 5000,10000    Escalones a medida
    ./webbench.py --slo-p99-ms 5        SLO distinto
    ./webbench.py --export-md FILE      Exporta la tabla (con bloque de entorno)

    # Generador en OTRA máquina (la única forma de obtener cifras citables):
    ./webbench.py --bind 10.0.0.10 --generator-host roberto@10.0.0.20
    ./webbench.py --bind 10.0.0.10 --generator-host 10.0.0.20 \
                  --ssh-user roberto -i ~/.ssh/id_bench
"""

import argparse
import collections
import json
import os
import resource
import shlex
import shutil
import socket
import subprocess
import sys
import time
import urllib.request

DIR = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(os.path.dirname(DIR), "poly"))
import benchlib  # noqa: E402  (tras el sys.path: vive en benchmarks/poly/)

# Cada implementación con su comando ya construido y su puerto propio. Puertos distintos para
# que un TIME_WAIT del anterior no retrase al siguiente.
IMPLS = [
    ("ray", ["{dir}/ray/plaintext-ray", "{bind}", "{port}"], 18080),
    ("hyper", ["{dir}/hyper/target/release/plaintext-hyper", "{bind}", "{port}"], 18081),
    ("go", ["{dir}/go/plaintext-go", "{bind}", "{port}"], 18082),
    ("node", ["node", "{dir}/node/main.js", "{bind}", "{port}"], 18083),
]

# Techo de fds que se pide para el arnés y, por herencia, para los cuatro servidores.
FD_TARGET = 65536

# El generador remoto: destino ya en el formato de ssh ([usuario@]host), llave opcional, y la
# ruta de oha ALLÍ (absoluta; la resuelve el preflight). None = el generador corre aquí.
Generator = collections.namedtuple("Generator", "host key oha")

# Dónde buscar oha en el generador. Hace falta porque una shell de SSH no interactiva trae un
# PATH mínimo (`/usr/bin:/bin:/usr/sbin:/sbin` en macOS) que NO incluye Homebrew ni cargo: un
# `oha` pelado falla aunque esté perfectamente instalado.
REMOTE_OHA_CANDIDATES = [
    "/opt/homebrew/bin/oha",   # Homebrew en Apple Silicon
    "/usr/local/bin/oha",      # Homebrew en Intel, o instalación manual
    "$HOME/.cargo/bin/oha",    # cargo install oha
]

EXPECTED_BODY = b"Hello, World!"

# La escalera por defecto llega hasta donde las cuatro implementaciones ya han tocado techo
# en loopback (hyper, el techo de I/O, satura sobre 160k). Una escalera que se queda corta
# devuelve "todas empatadas en el último escalón", que no es un resultado: es no haber
# encontrado la rodilla. Cuando ya sabes el vecindario, afina con --rates.
DEFAULT_RATES = [5000, 10000, 20000, 40000, 80000, 120000, 160000, 200000]


def raise_fd_limit():
    """Sube el límite blando de fds del arnés; los servidores lo HEREDAN al hacer fork.

    No es cosmético, es una corrección de SESGO: raylang sube su propio límite blando al duro
    al arrancar (`src/lib.rs`), y el runtime de Go también (1.19+), pero **hyper y node no**.
    Lanzado desde una shell con el default de macOS (`ulimit -n 256`), ray y Go correrían con
    ~138 000 fds y hyper/node con 256 — desigualdad invisible en el resultado, que castiga a
    dos de las cuatro en cuanto sube la concurrencia. Igualándolo aquí, el veredicto deja de
    depender de la terminal desde la que se lanzó.
    """
    soft, hard = resource.getrlimit(resource.RLIMIT_NOFILE)
    if soft >= FD_TARGET:
        return soft
    target = FD_TARGET if hard == resource.RLIM_INFINITY else min(FD_TARGET, hard)
    try:
        resource.setrlimit(resource.RLIMIT_NOFILE, (target, hard))
    except (ValueError, OSError):
        return soft
    return resource.getrlimit(resource.RLIMIT_NOFILE)[0]


def wait_ready(host, port, timeout_s=10.0):
    """Espera a que el puerto ACEPTE conexiones. Un sleep fijo mediría el arranque de unos y
    no el de otros (node tarda ~40 ms, un binario nativo ~3 ms); esperar el evento real hace
    la comparación honesta y además falla rápido si el servidor no levanta."""
    deadline = time.monotonic() + timeout_s
    while time.monotonic() < deadline:
        try:
            with socket.create_connection((host, port), timeout=0.25):
                return True
        except OSError:
            time.sleep(0.02)
    return False


def wait_port_free(host, port, timeout_s=10.0):
    """Espera a que el puerto deje de aceptar (el proceso murió del todo). Sin esto, la
    siguiente implementación puede encontrarse el bind ocupado o —peor— medir contra el
    servidor anterior todavía vivo."""
    deadline = time.monotonic() + timeout_s
    while time.monotonic() < deadline:
        try:
            with socket.create_connection((host, port), timeout=0.25):
                time.sleep(0.05)
        except OSError:
            return True
    return False


def check_response(host, port):
    """Verifica cuerpo y status. El equivalente del checksum del banco poliglota: dos
    servidores que no responden lo mismo no son comparables.

    Se comprueba SIEMPRE desde esta máquina (aunque el generador sea remoto): es un chequeo
    de corrección, no de rendimiento, y hacerlo local mantiene el diagnóstico simple."""
    try:
        with urllib.request.urlopen(f"http://{host}:{port}/", timeout=5) as r:
            body = r.read()
            if r.status != 200:
                return f"status {r.status}, esperaba 200"
            if body != EXPECTED_BODY:
                return f"cuerpo {body!r}, esperaba {EXPECTED_BODY!r}"
    except Exception as e:  # noqa: BLE001 — cualquier fallo aquí invalida la medición
        return f"no responde: {e}"
    return None


def ssh_argv(generator, remote_command):
    """El `ssh` para ejecutar `remote_command` en el generador.

    `BatchMode=yes` para que un SSH mal configurado falle en el acto en vez de quedarse
    esperando una passphrase en medio de una sesión de medida. Con `--ssh-key` se añade
    `IdentitiesOnly=yes`: si no, ssh ofrece primero las claves del agente y la `-i` explícita
    podría no llegar a usarse nunca — el flag diría una cosa y la conexión haría otra.
    """
    argv = ["ssh", "-o", "BatchMode=yes"]
    if generator.key:
        argv += ["-i", generator.key, "-o", "IdentitiesOnly=yes"]
    return argv + [generator.host, remote_command]


def resolve_remote_oha(generator):
    """La ruta absoluta de oha en el generador, o None si no aparece.

    Prueba, en orden: el PATH tal cual, un shell de LOGIN (que sí carga el perfil del usuario),
    y las rutas conocidas. Resolverlo UNA vez aquí evita depender del PATH de cada `ssh`
    posterior, que en una shell no interactiva de macOS es solo `/usr/bin:/bin:/usr/sbin:/sbin`
    — sin Homebrew ni cargo, donde vive oha en la práctica.
    """
    probe = "; ".join([
        "command -v oha 2>/dev/null && exit 0",
        "sh -lc 'command -v oha' 2>/dev/null && exit 0",
        *(f'[ -x "{p}" ] && echo "{p}" && exit 0' for p in REMOTE_OHA_CANDIDATES),
        "exit 1",
    ])
    out = subprocess.run(ssh_argv(generator, probe), capture_output=True, text=True)
    if out.returncode != 0:
        return None
    path = out.stdout.strip().splitlines()
    return path[0].strip() if path else None


def oha_command(target, port, rate, conns, duration_s, generator, correction=False):
    """El comando de oha, local (`generator` None) o vía SSH.

    Remoto: se envuelve en la shell del `ssh` para poder subir el `ulimit -n` del generador en
    la MISMA shell que lanza oha (con -c 200 y el default de macOS de 256 fds, oha se queda al
    borde y empieza a fallar conexiones — que el arnés leería como "el servidor no sostiene la
    tasa", atribuyendo al servidor un límite del generador)."""
    oha = [
        generator.oha if generator else "oha",
        "--no-tui", "--output-format", "json",
        "-z", f"{duration_s}s", "-c", str(conns), "-q", str(rate),
        *(["--latency-correction"] if correction else []),
        f"http://{target}:{port}/",
    ]
    if generator is None:
        return oha
    remote = f"ulimit -n {FD_TARGET} 2>/dev/null; exec " + " ".join(shlex.quote(a) for a in oha)
    return ssh_argv(generator, remote)


def run_oha(target, port, rate, conns, duration_s, generator=None, correction=False):
    """Una corrida de oha a tasa FIJA. Devuelve el dict de métricas o None si falló."""
    cmd = oha_command(target, port, rate, conns, duration_s, generator, correction)
    out = subprocess.run(cmd, capture_output=True, text=True)
    if out.returncode != 0:
        return None
    try:
        data = json.loads(out.stdout)
    except json.JSONDecodeError:
        return None
    pct = data.get("latencyPercentiles", {})
    return {
        "rps": data["summary"]["requestsPerSec"],
        "success_rate": data["summary"]["successRate"],
        "p50": pct.get("p50", 0) * 1000.0,
        "p99": pct.get("p99", 0) * 1000.0,
        "p999": pct.get("p99.9", 0) * 1000.0,
    }


def start_server(name, cmd_template, port, bind, check_host):
    """Levanta una implementación y devuelve su proceso, o None si no sirve para medir.
    Verifica la respuesta ANTES de que nadie mida: dos servidores que no responden lo mismo
    no son comparables (el equivalente del checksum del banco poliglota)."""
    cmd = [part.format(dir=os.path.join(DIR, "plaintext"), bind=bind, port=port)
           for part in cmd_template]
    exe = cmd[0]
    if not os.path.exists(exe) and "/" in exe:
        print(f">> {name}: falta {exe} — corre ./build-all.sh", file=sys.stderr)
        return None

    proc = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    if not wait_ready(check_host, port):
        err = (proc.stderr.read(4096) or b"").decode(errors="replace").strip()
        print(f">> {name}: no aceptó conexiones en 10 s. {err}", file=sys.stderr)
        stop_server(proc, check_host, port)
        return None

    problem = check_response(check_host, port)
    if problem:
        print(f">> {name}: respuesta no comparable — {problem}", file=sys.stderr)
        stop_server(proc, check_host, port)
        return None
    return proc


def stop_server(proc, host, port):
    proc.terminate()
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait()
    wait_port_free(host, port)


def run_ladder(live, rates, conns, duration_s, warmup_s, slo_p99_ms, target, generator,
               correction=False):
    """Escalera INTERCALADA con rotación: en cada escalón de tasa se miden todas las
    implementaciones vivas, y el orden rota (A B C / B C A / C A B ...).

    Por qué, y no una implementación entera de una tirada: el drift ambiental (térmico,
    procesos de fondo) se reparte entre todas en vez de caer entero sobre la que tocaba en
    ese momento, y la rotación cancela el sesgo de posición dentro del escalón. Es la misma
    disciplina del banco poliglota (`poly/benchlib.run_variants`), y la lección que
    `docs/investigacion-p99-framework-web.md` §12 pagó cara: con corridas consecutivas, el
    ORDEN llegó a determinar el resultado por completo y a invertir el signo de la conclusión.

    Todos los servidores viven a la vez, cada uno en su puerto; solo uno recibe carga en cada
    momento y los demás están ociosos (coste de CPU ~0). Así no hay arranques y paradas entre
    escalones, que es justo lo que §12 identificó como fuente de sesgo (TIME_WAIT y limpieza
    del kernel con gaps cortos).
    """
    steps = {name: [] for name, _, _ in live}
    done = set()

    for name, _proc, port in live:
        print(f">> {name}: calentando {warmup_s}s...", file=sys.stderr)
        run_oha(target, port, max(rates), conns, warmup_s, generator, correction)

    for i, rate in enumerate(rates):
        active = [t for t in live if t[0] not in done]
        if not active:
            break
        pivot = i % len(active)
        for name, _proc, port in active[pivot:] + active[:pivot]:
            print(f">> -q {rate}: {name} ({duration_s}s)...", file=sys.stderr)
            m = run_oha(target, port, rate, conns, duration_s, generator, correction)
            if m is None:
                print(f">> {name}: oha falló a -q {rate}", file=sys.stderr)
                continue
            m["rate"] = rate
            # "Sostiene la tasa" = consigue >=99% de lo pedido. Por debajo de eso el servidor
            # ya no da abasto y su p99 describe otro régimen, no el que se pidió medir.
            m["sustained"] = m["rps"] >= rate * 0.99 and m["success_rate"] >= 0.999
            m["within_slo"] = m["p99"] <= slo_p99_ms
            steps[name].append(m)
            if not m["sustained"]:
                # Techo encontrado: los escalones por encima solo repiten el mismo régimen de
                # saturación (la tasa conseguida se queda clavada y la latencia crece con el
                # encolamiento). Seguir subiendo no añade información, solo minutos.
                print(f">> {name}: techo en ~{m['rps']:,.0f} rps, escalera cortada", file=sys.stderr)
                done.add(name)
    return steps


def verdict(steps):
    """La tasa más alta que sostiene el SLO. None si ni el primer escalón lo cumple."""
    ok = [s for s in steps if s["sustained"] and s["within_slo"]]
    return max(ok, key=lambda s: s["rate"]) if ok else None


HEADERS = ("Implementación", "Tasa sostenida bajo SLO", "p50", "p99", "p99.9", "Primer escalón fallido")


def _why_failed(failed):
    """Por qué se cayó el primer escalón que no pasó. Se mira PRIMERO si sostuvo la tasa: un
    servidor que no llega a la tasa pedida tocó su techo de throughput, y su p99 en ese
    escalón (segundos, por el encolamiento) describe el régimen de saturación, no una cola
    patológica — reportarlo como 'p99 alto' confundiría dos fallos distintos."""
    if failed is None:
        return "-"
    if not failed["sustained"]:
        return f"{failed['rate']:,} rps (techo: solo {failed['rps']:,.0f})"
    return f"{failed['rate']:,} rps (p99 {failed['p99']:.1f} ms)"


def build_rows(results, slo_p99_ms):
    ranked, failures = [], []
    for name, steps in results:
        if not steps:
            failures.append([name, "sin datos", "-", "-", "-", "-"])
            continue
        best = verdict(steps)
        why = _why_failed(next((s for s in steps if not (s["sustained"] and s["within_slo"])), None))
        if best:
            ranked.append((name, best, why))
        else:
            first = steps[0]
            failures.append([name, f"ninguna (ya falla en {first['rate']:,})",
                             f"{first['p50']:.2f} ms", f"{first['p99']:.2f} ms",
                             f"{first['p999']:.2f} ms", why])

    ranked.sort(key=lambda r: r[1]["rate"], reverse=True)
    top = ranked[0][1]["rate"] if ranked else None
    rows = [[name, f"{b['rate']:,} rps ({b['rate'] / top:.2f}x líder)", f"{b['p50']:.2f} ms",
             f"{b['p99']:.2f} ms", f"{b['p999']:.2f} ms", why]
            for name, b, why in ranked]
    return rows + failures


def print_ladder(name, steps):
    print(f"\n=== {name} — escalones ===")
    ladder = [["tasa pedida", "conseguida", "p50", "p99", "p99.9", "veredicto"]]
    for s in steps:
        mark = "ok" if (s["sustained"] and s["within_slo"]) else ("no sostiene" if not s["sustained"] else "fuera de SLO")
        ladder.append([f"{s['rate']:,}", f"{s['rps']:,.0f}", f"{s['p50']:.2f} ms",
                       f"{s['p99']:.2f} ms", f"{s['p999']:.2f} ms", mark])
    benchlib.print_table(ladder[1:], headers=tuple(ladder[0]))


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--only", help="implementaciones a medir, separadas por coma")
    ap.add_argument("--rates", help="escalones de tasa, separados por coma")
    ap.add_argument("--connections", "-c", type=int, default=100, help="conexiones concurrentes (default: 100)")
    ap.add_argument("--duration", "-z", type=int, default=10, help="segundos por escalón (default: 10)")
    ap.add_argument("--warmup", type=int, default=3, help="segundos de calentamiento descartado (default: 3)")
    ap.add_argument("--slo-p99-ms", type=float, default=10.0, help="SLO de p99 en ms (default: 10)")
    ap.add_argument("--bind", default="127.0.0.1",
                    help="IP a la que bindean los servidores (default: 127.0.0.1). Con generador "
                         "remoto, la IP del enlace, p. ej. 10.0.0.10")
    ap.add_argument("--generator-host",
                    help="host SSH donde corre oha, con el formato de ssh ([usuario@]host). "
                         "Default: esta máquina. Con esto las cifras dejan de estar contaminadas "
                         "por el generador; ver README §Loopback")
    ap.add_argument("--ssh-user", metavar="USUARIO",
                    help="usuario del SSH al generador. Alternativa a escribirlo en "
                         "--generator-host (usuario@host); si el host ya lo trae, gana el host")
    ap.add_argument("--ssh-key", "-i", metavar="RUTA",
                    help="llave privada para el SSH al generador (como `ssh -i`). Sin esto se usa "
                         "la configuración de ssh del usuario (~/.ssh/config, agente)")
    ap.add_argument("--correction", action="store_true",
                    help="añade --latency-correction a oha. OPT-IN por medición: con -q el run ya "
                         "es open-loop y la corrección cobra como latencia del servidor el jitter "
                         "de planificación del generador (ver README §La corrección de latencia)")
    ap.add_argument("--remote-oha", metavar="RUTA",
                    help="ruta de oha en el generador. Por defecto la resuelve el preflight "
                         "(PATH, shell de login, y las rutas de Homebrew/cargo)")
    ap.add_argument("--export-md", help="exporta la tabla a Markdown (append)")
    args = ap.parse_args()

    remote = bool(args.generator_host)
    for flag, value in (("--ssh-user", args.ssh_user), ("--ssh-key", args.ssh_key),
                        ("--remote-oha", args.remote_oha)):
        if value and not remote:
            print(f"error: {flag} solo tiene sentido con --generator-host", file=sys.stderr)
            return 1
    if not remote and not shutil.which("oha"):
        print("Falta oha: brew install oha  (o https://github.com/hatoo/oha)", file=sys.stderr)
        return 127

    key = os.path.expanduser(args.ssh_key) if args.ssh_key else None
    if key and not os.path.exists(key):
        print(f"error: no existe la llave {args.ssh_key}", file=sys.stderr)
        return 1
    # `usuario@host` explícito en --generator-host manda sobre --ssh-user: es lo que ssh
    # entiende y lo que el usuario tecleó de forma más específica.
    destination = args.generator_host
    if remote and args.ssh_user and "@" not in destination:
        destination = f"{args.ssh_user}@{destination}"
    generator = Generator(destination, key, args.remote_oha) if remote else None

    # El chequeo local a 0.0.0.0 no vale; el bind comodín se comprueba por loopback.
    check_host = "127.0.0.1" if args.bind in ("0.0.0.0", "::", "") else args.bind
    # Lo que oha pone en la URL: la IP del bind (alcanzable desde el generador remoto).
    target = args.bind if args.bind not in ("0.0.0.0", "::", "") else "127.0.0.1"

    if remote:
        # Preflight: sin esto, un SSH mal configurado o un oha que el PATH no ve se
        # manifestarían como "ninguna implementación sostiene ninguna tasa", el peor
        # diagnóstico posible. Resuelve además la ruta absoluta de oha una sola vez.
        if not generator.oha:
            found = resolve_remote_oha(generator)
            if not found:
                print(f"error: no se pudo ejecutar oha en {destination} vía SSH.\n"
                      "  · ¿llave correcta y sin passphrase, y usuario remoto correcto?\n"
                      "  · ¿oha instalado allí? Ojo: una shell de SSH no interactiva trae un\n"
                      "    PATH mínimo sin Homebrew, así que un oha instalado puede no verse.\n"
                      "    Pásalo explícito con --remote-oha /opt/homebrew/bin/oha.",
                      file=sys.stderr)
                return 127
            generator = generator._replace(oha=found)
        if args.bind in ("127.0.0.1", "localhost"):
            print(f"error: --generator-host {destination} con --bind {args.bind}: el "
                  "generador remoto no puede alcanzar loopback. Pasa la IP del enlace "
                  "(p. ej. --bind 10.0.0.10).", file=sys.stderr)
            return 1

    rates = [int(r) for r in args.rates.split(",")] if args.rates else DEFAULT_RATES
    only = set(args.only.split(",")) if args.only else None
    impls = [i for i in IMPLS if only is None or i[0] in only]
    if not impls:
        print(f"error: ninguna implementación casa con --only {args.only}", file=sys.stderr)
        return 1

    fds = raise_fd_limit()
    print(f">> plaintext · -c {args.connections} · {args.duration}s por escalón · SLO p99 <= {args.slo_p99_ms} ms",
          file=sys.stderr)
    print(f">> bind {args.bind} · fds {fds} (heredados por los servidores) · generador: "
          f"{destination + ' (' + generator.oha + ')' if remote else 'local'}", file=sys.stderr)
    if fds < args.connections * 4:
        print(f">> AVISO: {fds} fds para -c {args.connections} es justo; hyper y node no suben "
              ">> su propio límite y podrían fallar conexiones.", file=sys.stderr)
    if not remote:
        print(">> AVISO: el generador corre en ESTA máquina y compite por los mismos cores que\n"
              ">> el servidor. Sirve para depurar el arnés y para comparaciones relativas, NO\n"
              ">> para publicar cifras (ver README.md §Loopback).", file=sys.stderr)

    live = []
    for name, cmd, port in impls:
        proc = start_server(name, cmd, port, args.bind, check_host)
        if proc:
            live.append((name, proc, port))
    if not live:
        print("error: ninguna implementación levantó", file=sys.stderr)
        return 1

    try:
        steps = run_ladder(live, rates, args.connections, args.duration, args.warmup,
                           args.slo_p99_ms, target, generator, args.correction)
    finally:
        for _name, proc, port in live:
            stop_server(proc, check_host, port)

    # Las que no levantaron quedan con lista vacía → "sin datos" en la tabla, no desaparecen.
    results = [(name, steps.get(name, [])) for name, _, _ in impls]
    for name, s in results:
        if s:
            print_ladder(name, s)

    print(f"\n=== plaintext — veredicto (tasa sostenida con p99 <= {args.slo_p99_ms} ms) ===")
    rows = build_rows(results, args.slo_p99_ms)
    benchlib.print_table(rows, headers=HEADERS)

    if args.export_md:
        meta = benchlib.env_metadata(exclude=("lua", "php", "pl", "py", "rb"))
        oha_version = (benchlib._version_line(ssh_argv(generator, f"{generator.oha} --version")) if remote
                       else benchlib._version_line(["oha", "--version"]))
        meta.append(("oha", oha_version))
        meta.append(("carga", f"-c {args.connections}, {args.duration}s/escalón, SLO p99 <= {args.slo_p99_ms} ms"))
        meta.append(("fds", str(fds)))
        meta.append(("latency-correction", "sí" if args.correction else "no (default)"))
        # El origen del generador va SIEMPRE en el bloque de entorno: es lo que decide si las
        # cifras son citables, y un export suelto no tiene otra forma de decirlo.
        meta.append(("generador", f"remoto vía SSH: {destination} → {target}" if remote
                     else "loopback (misma máquina) — NO publicable, ver README §Loopback"))
        benchlib.write_markdown_metadata(args.export_md, meta)
        benchlib.write_markdown(args.export_md, "plaintext — veredicto", rows, headers=HEADERS)
    return 0


if __name__ == "__main__":
    sys.exit(main())
