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

`oha` se invoca SIEMPRE con `-q` (tasa fija, open-loop) y `--latency-correction`. Sin `-q`,
oha es closed-loop: cuando el servidor se atasca el generador deja de mandar y el stall nunca
se registra — los p99 salen bonitos justo cuando el sistema está peor (coordinated omission).
Por eso la tasa no es un flag opcional del arnés sino el eje del experimento.

Uso:
    ./webbench.py                       Mide todas las implementaciones de `plaintext`
    ./webbench.py --only ray,hyper      Solo esas
    ./webbench.py --rates 5000,10000    Escalones a medida
    ./webbench.py --slo-p99-ms 5        SLO distinto
    ./webbench.py --export-md FILE      Exporta la tabla (con bloque de entorno)
"""

import argparse
import json
import os
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
    ("ray", ["{dir}/ray/plaintext-ray", "{port}"], 18080),
    ("hyper", ["{dir}/hyper/target/release/plaintext-hyper", "{port}"], 18081),
    ("go", ["{dir}/go/plaintext-go", "{port}"], 18082),
    ("node", ["node", "{dir}/node/main.js", "{port}"], 18083),
]

EXPECTED_BODY = b"Hello, World!"

# La escalera por defecto llega hasta donde las cuatro implementaciones ya han tocado techo
# en loopback (hyper, el techo de I/O, satura sobre 160k). Una escalera que se queda corta
# devuelve "todas empatadas en el último escalón", que no es un resultado: es no haber
# encontrado la rodilla. Cuando ya sabes el vecindario, afina con --rates.
DEFAULT_RATES = [5000, 10000, 20000, 40000, 80000, 120000, 160000, 200000]


def wait_ready(port, timeout_s=10.0):
    """Espera a que el puerto ACEPTE conexiones. Un sleep fijo mediría el arranque de unos y
    no el de otros (node tarda ~40 ms, un binario nativo ~3 ms); esperar el evento real hace
    la comparación honesta y además falla rápido si el servidor no levanta."""
    deadline = time.monotonic() + timeout_s
    while time.monotonic() < deadline:
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=0.25):
                return True
        except OSError:
            time.sleep(0.02)
    return False


def wait_port_free(port, timeout_s=10.0):
    """Espera a que el puerto deje de aceptar (el proceso murió del todo). Sin esto, la
    siguiente implementación puede encontrarse el bind ocupado o —peor— medir contra el
    servidor anterior todavía vivo."""
    deadline = time.monotonic() + timeout_s
    while time.monotonic() < deadline:
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=0.25):
                time.sleep(0.05)
        except OSError:
            return True
    return False


def check_response(port):
    """Verifica cuerpo y status. El equivalente del checksum del banco poliglota: dos
    servidores que no responden lo mismo no son comparables."""
    try:
        with urllib.request.urlopen(f"http://127.0.0.1:{port}/", timeout=5) as r:
            body = r.read()
            if r.status != 200:
                return f"status {r.status}, esperaba 200"
            if body != EXPECTED_BODY:
                return f"cuerpo {body!r}, esperaba {EXPECTED_BODY!r}"
    except Exception as e:  # noqa: BLE001 — cualquier fallo aquí invalida la medición
        return f"no responde: {e}"
    return None


def run_oha(port, rate, conns, duration_s):
    """Una corrida de oha a tasa FIJA. Devuelve el dict de métricas o None si falló."""
    cmd = [
        "oha", "--no-tui", "--output-format", "json",
        "-z", f"{duration_s}s", "-c", str(conns), "-q", str(rate),
        "--latency-correction",
        f"http://127.0.0.1:{port}/",
    ]
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


def start_server(name, cmd_template, port):
    """Levanta una implementación y devuelve su proceso, o None si no sirve para medir.
    Verifica la respuesta ANTES de que nadie mida: dos servidores que no responden lo mismo
    no son comparables (el equivalente del checksum del banco poliglota)."""
    cmd = [part.format(dir=os.path.join(DIR, "plaintext"), port=port) for part in cmd_template]
    exe = cmd[0]
    if not os.path.exists(exe) and "/" in exe:
        print(f">> {name}: falta {exe} — corre ./build-all.sh", file=sys.stderr)
        return None

    proc = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    if not wait_ready(port):
        err = (proc.stderr.read(4096) or b"").decode(errors="replace").strip()
        print(f">> {name}: no aceptó conexiones en 10 s. {err}", file=sys.stderr)
        stop_server(proc, port)
        return None

    problem = check_response(port)
    if problem:
        print(f">> {name}: respuesta no comparable — {problem}", file=sys.stderr)
        stop_server(proc, port)
        return None
    return proc


def stop_server(proc, port):
    proc.terminate()
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait()
    wait_port_free(port)


def run_ladder(live, rates, conns, duration_s, warmup_s, slo_p99_ms):
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
        run_oha(port, max(rates), conns, warmup_s)

    for i, rate in enumerate(rates):
        active = [t for t in live if t[0] not in done]
        if not active:
            break
        pivot = i % len(active)
        for name, _proc, port in active[pivot:] + active[:pivot]:
            print(f">> -q {rate}: {name} ({duration_s}s)...", file=sys.stderr)
            m = run_oha(port, rate, conns, duration_s)
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
    ap.add_argument("--export-md", help="exporta la tabla a Markdown (append)")
    args = ap.parse_args()

    if not shutil.which("oha"):
        print("Falta oha: brew install oha  (o https://github.com/hatoo/oha)", file=sys.stderr)
        return 127

    rates = [int(r) for r in args.rates.split(",")] if args.rates else DEFAULT_RATES
    only = set(args.only.split(",")) if args.only else None
    impls = [i for i in IMPLS if only is None or i[0] in only]
    if not impls:
        print(f"error: ninguna implementación casa con --only {args.only}", file=sys.stderr)
        return 1

    print(f">> plaintext · -c {args.connections} · {args.duration}s por escalón · SLO p99 <= {args.slo_p99_ms} ms",
          file=sys.stderr)
    print(">> AVISO: si el generador corre en ESTA máquina, compite por los mismos cores que el\n"
          ">> servidor. Sirve para depurar el arnés y para comparaciones relativas, NO para\n"
          ">> publicar cifras (ver README.md §Loopback).", file=sys.stderr)

    live = []
    for name, cmd, port in impls:
        proc = start_server(name, cmd, port)
        if proc:
            live.append((name, proc, port))
    if not live:
        print("error: ninguna implementación levantó", file=sys.stderr)
        return 1

    try:
        steps = run_ladder(live, rates, args.connections, args.duration, args.warmup,
                           args.slo_p99_ms)
    finally:
        for _name, proc, port in live:
            stop_server(proc, port)

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
        meta.append(("oha", benchlib._version_line(["oha", "--version"])))
        meta.append(("carga", f"-c {args.connections}, {args.duration}s/escalón, SLO p99 <= {args.slo_p99_ms} ms"))
        meta.append(("generador", "loopback (misma máquina) — no publicable, ver README §Loopback"))
        benchlib.write_markdown_metadata(args.export_md, meta)
        benchlib.write_markdown(args.export_md, "plaintext — veredicto", rows, headers=HEADERS)
    return 0


if __name__ == "__main__":
    sys.exit(main())
