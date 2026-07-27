#!/usr/bin/env python3
"""benchlib — utilidades compartidas por mem-bench.py y ray-bench.py."""

import os
import re
import shutil
import signal
import statistics
import subprocess
import sys
import threading
import time
import tomllib

SETTINGS_FILE = "settings.toml"

RUNNERS = {
    "js": ["node"],
    "lua": ["lua"],
    "php": ["php"],
    "pl": ["perl"],
    "py": ["python3"],
    "ray": ["ray"],
    "rb": ["ruby"],
}

# Categoría + descripción de cada programa (orden = orden de agrupación en
# list/choose y en el menú de la TUI). Un programa sin entrada acá cae en
# "Otros" con descripción vacía — no rompe el descubrimiento por directorio.
CATEGORIES = [
    ("Arranque", [
        ("empty", "programa vacío — overhead puro de arranque del runtime/intérprete"),
        ("print", "un solo print — arranque + I/O mínimo"),
    ]),
    ("CPU / aritmética-recursión", [
        ("loopsum", "suma con módulo en un loop de 10M iteraciones — aritmética entera pura"),
        ("fibonacci", "fibonacci recursivo para n=0..9 — llamadas a función, poco trabajo"),
        ("fibrec", "fibonacci recursivo profundo fib(34) — stress de llamadas a función"),
        ("factorial", "factorial recursivo para n=0..9 — recursión simple"),
    ]),
    ("Datos de servicio (stdlib string + hashmap)", [
        ("wordcount", "contar frecuencias de palabras — split + hashmap + sort"),
        ("jsonserialize", "serializar N registros a JSON — construcción de strings (ruta de salida)"),
        ("jsondeserialize", "parsear JSON — búsqueda de substrings + parse_int (ruta de entrada)"),
        ("logparse", "parsear líneas de log — split + parse_int + agregación en dos hashmaps"),
    ]),
    ("Estructuras de datos / GC", [
        ("treealloc", "árboles binarios — construir/contar/descartar muchos nodos (presión de GC/allocator)"),
    ]),
    ("Numérico", [
        ("sortnums", "ordenar 1M de enteros generados con un LCG determinista"),
        ("matrixmul", "multiplicación de matrices 200×200 — único workload con punto flotante"),
    ]),
    ("Pattern matching", [
        ("regex", "extracción de campos con motor de regex nativo"),
    ]),
]


def describe(name):
    for _, items in CATEGORIES:
        for n, desc in items:
            if n == name:
                return desc
    return ""


def grouped_names(names):
    """Agrupa `names` (los realmente presentes en el directorio) según
    CATEGORIES, preservando su orden; los no catalogados van al final en
    'Otros'."""
    known = {n for _, items in CATEGORIES for n, _ in items}
    groups = []
    for cat, items in CATEGORIES:
        present = [n for n, _ in items if n in names]
        if present:
            groups.append((cat, present))
    leftover = [n for n in names if n not in known]
    if leftover:
        groups.append(("Otros", leftover))
    return groups


def load_disabled_languages(base_dir):
    """Lee settings.toml (si existe) y devuelve el set de claves con valor false."""
    path = os.path.join(base_dir, SETTINGS_FILE)
    if not os.path.isfile(path):
        return set()
    with open(path, "rb") as f:
        config = tomllib.load(f)
    return {lang for lang, enabled in config.items() if not enabled}


def resolve_exclude(base_dir, cli_exclude):
    """Combina settings.toml con el --exclude de la línea de comandos."""
    return load_disabled_languages(base_dir) | set(cli_exclude.split())


def _replace_toml_value(line, key, value):
    """Si la línea asigna `key`, reemplaza el valor conservando el comentario
    inline y la alineación; si no, devuelve None."""
    m = re.match(r"^(\s*)" + re.escape(key) + r"(\s*=\s*)([^#\n]*?)(\s*)(#.*)?$", line)
    if m is None:
        return None
    indent, eq, old, gap, comment = m.groups()
    new = str(value).lower() if isinstance(value, bool) else str(value)
    if not comment:
        return f"{indent}{key}{eq}{new}\n"
    # Mantiene la columna del comentario: ajusta el gap según cambie el ancho.
    pad = " " * max(1, len(old) + len(gap) - len(new))
    return f"{indent}{key}{eq}{new}{pad}{comment}\n"


def save_config(base_dir, languages, bench):
    """Persiste en settings.toml el estado de las variantes ({clave: bool}) y
    la tabla [bench] ({runs, warmup}), editando el archivo línea a línea para
    conservar comentarios y orden. Las claves que no existan se agregan."""
    path = os.path.join(base_dir, SETTINGS_FILE)
    lines = []
    if os.path.isfile(path):
        with open(path, encoding="utf-8") as f:
            lines = f.readlines()

    pending_langs = dict(languages)
    pending_bench = dict(bench)
    section = None  # None = raíz (variantes); "bench" = tabla [bench]
    last_root_assign = -1  # índice en `out` de la última asignación de la raíz
    out = []
    for line in lines:
        m = re.match(r"^\s*\[(.+?)\]", line)
        if m:
            section = m.group(1)
            out.append(line)
            continue
        pending = pending_langs if section is None else pending_bench if section == "bench" else {}
        for key in list(pending):
            replaced = _replace_toml_value(line, key, pending[key])
            if replaced is not None:
                out.append(replaced)
                del pending[key]
                break
        else:
            out.append(line)
        if section is None and re.match(r"^\s*\w[\w-]*\s*=", line):
            last_root_assign = len(out) - 1

    # Variantes que no estaban en el archivo: van tras la última asignación de
    # la raíz (no al final, que podría caer dentro de [bench]).
    for i, (key, val) in enumerate(pending_langs.items()):
        out.insert(last_root_assign + 1 + i, f"{key} = {str(val).lower()}\n")
    if pending_bench:
        if section != "bench":
            out.append("\n[bench]\n")
        for key, val in pending_bench.items():
            out.append(f"{key} = {val}\n")

    with open(path, "w", encoding="utf-8") as f:
        f.writelines(out)


BENCH_DEFAULTS = {"runs": 20, "warmup": 10, "budget_s": 5}


def load_bench_defaults(base_dir):
    """Lee la tabla [bench] de settings.toml (runs/warmup); si falta, usa BENCH_DEFAULTS."""
    defaults = dict(BENCH_DEFAULTS)
    path = os.path.join(base_dir, SETTINGS_FILE)
    if not os.path.isfile(path):
        return defaults
    with open(path, "rb") as f:
        config = tomllib.load(f)
    bench = config.get("bench", {})
    for key in defaults:
        if key in bench:
            defaults[key] = bench[key]
    return defaults


def discover_names(base_dir):
    names = []
    for entry in sorted(os.listdir(base_dir)):
        if entry.startswith(".") or entry == "__pycache__":
            continue
        if os.path.isdir(os.path.join(base_dir, entry)):
            names.append(entry)
    if not names:
        sys.exit(f"error: no hay programas en {base_dir}")
    return names


def variants_for(base_dir, name, exclude):
    """Variantes ejecutables de un programa: [(etiqueta, cmd_list), ...]."""
    base = os.path.join(base_dir, name)
    variants = []

    native = os.path.join(base, name)
    if "native" not in exclude and os.path.isfile(native) and os.access(native, os.X_OK):
        variants.append((f"{name}.native", [native]))

    for entry in sorted(os.listdir(base)):
        path = os.path.join(base, entry)
        if not os.path.isfile(path) or entry == name:
            continue
        root, dot, ext = entry.rpartition(".")
        if not dot or root != name or ext in exclude:
            continue

        if ext in ("go", "rs"):
            suffix = "go" if ext == "go" else "rs"
            compiled = os.path.join(base, f"{name}-{suffix}")
            if os.path.isfile(compiled) and os.access(compiled, os.X_OK):
                variants.append((f"{name}.{ext}", [compiled]))
            else:
                print(f">> Omitido {name}.{ext} (binario {name}-{suffix} no compilado)", file=sys.stderr)
            continue

        runner = RUNNERS.get(ext)
        if not runner or shutil.which(runner[0]) is None:
            if runner:
                print(f">> Omitido {name}.{ext} (intérprete '{runner[0]}' no encontrado)", file=sys.stderr)
            continue
        variants.append((f"{name}.{ext}", runner + [path]))

    return variants


def human_time(t):
    t = float(t)
    if t < 1e-3:
        return f"{t * 1e6:.1f} µs"
    if t < 1:
        return f"{t * 1e3:.2f} ms"
    return f"{t:.3f} s"


def human_bytes(n):
    n = float(n)
    for unit in ("B", "KB", "MB", "GB"):
        if abs(n) < 1024:
            return f"{n:.1f} {unit}"
        n /= 1024
    return f"{n:.1f} TB"


BENCH_NS_RE = re.compile(rb"bench_ns=(\d+)")


def measure_once(cmd, prepare=None, check_cancel=None, poll_interval=0.05):
    """Ejecuta cmd una vez y devuelve (tiempo_s, pico_bytes, cpu_s).

    Spawn directo con posix_spawnp + wait4, sin shell ni wrapper tipo
    /usr/bin/time en el medio: el overhead ajeno al programa queda en el
    fork/exec del SO (<1 ms; con el wrapper eran ~3-5 ms, más que la señal en
    los binarios más rápidos). Del rusage del MISMO wait salen la memoria pico
    (ru_maxrss) y el tiempo de CPU (ru_utime+ru_stime): tiempo, memoria y CPU
    son siempre de la misma corrida. La CPU sirve de detector de interferencia
    (pared ≫ CPU en un workload CPU-bound = el SO desalojó al proceso).

    Auto-medición: si el programa emite `bench_ns=<int>` por stderr (su propio
    cronómetro monotónico alrededor del workload), ESE es el tiempo devuelto —
    mide solo cómputo, sin el arranque del runtime. Sin marcador, fallback al
    proceso completo (así los benchs de arranque miden arranque por diseño y
    la migración puede ser incremental). stdout no participa: la verificación
    de output por diff queda intacta.

    Si se pasa `check_cancel` (callable sin argumentos -> bool), en vez de
    esperar al proceso de un tirón se lo sondea cada `poll_interval` segundos;
    si `check_cancel()` da True en algún sondeo, se mata el proceso YA y se
    devuelve (None, None, None) para señalar cancelación.
    """
    if prepare:
        subprocess.run(prepare, shell=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    devnull = os.open(os.devnull, os.O_WRONLY)
    err_r, err_w = os.pipe()
    try:
        actions = [(os.POSIX_SPAWN_DUP2, devnull, 1), (os.POSIX_SPAWN_DUP2, err_w, 2)]
        start = time.perf_counter()
        pid = os.posix_spawnp(cmd[0], cmd, os.environ, file_actions=actions)
    finally:
        os.close(devnull)
        os.close(err_w)

    # Drenar stderr en paralelo al wait: si el hijo escribiera más que el buffer
    # del pipe (64 KB) y nadie leyera, se bloquearía y el wait no volvería nunca.
    stderr_chunks = []

    def drain():
        while True:
            chunk = os.read(err_r, 65536)
            if not chunk:
                break
            stderr_chunks.append(chunk)

    drainer = threading.Thread(target=drain, daemon=True)
    drainer.start()

    if check_cancel is None:
        _, _, rusage = os.wait4(pid, 0)
        elapsed = time.perf_counter() - start
    else:
        # El wait bloqueante va en un hilo y acá se sondea el Event: despierta
        # AL INSTANTE cuando el proceso termina (un sleep fijo redondearía toda
        # corrida hacia arriba al múltiplo de poll_interval) y aun así revisa
        # check_cancel cada poll_interval. El hilo toma el timestamp de fin al
        # salir de wait4, no cuando el sondeo lo nota.
        done = threading.Event()
        result = {}

        def reap():
            result["rusage"] = os.wait4(pid, 0)[2]
            result["end"] = time.perf_counter()
            done.set()

        threading.Thread(target=reap, daemon=True).start()
        while not done.wait(poll_interval):
            if check_cancel():
                try:
                    os.kill(pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass  # terminó justo ahora; el hilo lo cosecha igual
                done.wait()
                drainer.join()
                os.close(err_r)
                return None, None, None
        rusage = result["rusage"]
        elapsed = result["end"] - start

    drainer.join()
    os.close(err_r)
    marker = BENCH_NS_RE.search(b"".join(stderr_chunks))
    if marker:
        elapsed = int(marker.group(1)) / 1e9

    scale = 1 if sys.platform == "darwin" else 1024  # ru_maxrss: bytes en macOS, KB en Linux
    return elapsed, rusage.ru_maxrss * scale, rusage.ru_utime + rusage.ru_stime


# Corridas medidas mínimas por variante antes de que el presupuesto pueda
# cortarla: con menos de esto, mediana y MAD no significan nada.
MIN_BUDGET_RUNS = 5


def run_variants(variants, runs, warmup, prepare=None, progress=None, check_cancel=None,
                 budget_s=0.0):
    """Corre `variants` intercaladas por rondas con rotación (A B C / B C A /
    C A B ...): el drift ambiental (térmico, procesos de fondo) se reparte
    entre todas las variantes en vez de caer entero sobre la que tocaba en ese
    momento, y la rotación elimina el sesgo de posición dentro de la ronda.
    Cada variante corre `warmup` corridas descartadas y `runs` medidas.

    budget_s > 0 activa el presupuesto de tiempo por variante (estilo
    hyperfine): cuando el tiempo de PARED acumulado de una variante (proceso
    completo, warmup incluido — lo que de verdad cuesta la sesión) supera el
    presupuesto, deja de correr — pero nunca con menos de MIN_BUDGET_RUNS
    muestras medidas (y de 1 warmup). Las rápidas hacen sus `runs` completas;
    las lentas no marcan el ritmo de toda la sesión. budget_s=0 = sin límite.

    progress(label, counts, cancelled, completed): llamado tras cada corrida;
    `counts` es {label: corridas completadas} (incluye warmup); `completed` es
    el set de variantes que ya no correrán más (por runs o por presupuesto).
    check_cancel(label) -> bool: sondeado durante cada corrida; True excluye
    ESA variante de las rondas restantes (sus muestras se conservan).

    Devuelve (time_results, mem_results, cpu_results, cancelled); los tres
    primeros como [(label, muestras), ...] en el orden de `variants`.
    """
    times = {label: [] for label, _ in variants}
    mems = {label: [] for label, _ in variants}
    cpus = {label: [] for label, _ in variants}
    counts = {label: 0 for label, _ in variants}
    warmups_done = {label: 0 for label, _ in variants}
    spent = {label: 0.0 for label, _ in variants}  # pared real acumulada
    cancelled = set()
    completed = set()

    min_runs = min(MIN_BUDGET_RUNS, runs)
    over_budget = lambda label: budget_s > 0 and spent[label] >= budget_s

    round_idx = 0
    while True:
        active = [(l, c) for l, c in variants if l not in cancelled and l not in completed]
        if not active:
            break
        pivot = round_idx % len(active)
        for label, cmd in active[pivot:] + active[:pivot]:
            if label in cancelled or label in completed:
                continue
            cancel = (lambda l=label: check_cancel(l)) if check_cancel else None
            wall_start = time.perf_counter()
            t, m, c = measure_once(cmd, prepare, check_cancel=cancel)
            spent[label] += time.perf_counter() - wall_start
            if t is None:
                cancelled.add(label)
            else:
                counts[label] += 1
                # Fase warmup: se corre completa salvo que el presupuesto ya se
                # haya agotado (basta 1 warmup para calentar caches del SO).
                if warmups_done[label] < warmup and not (over_budget(label) and warmups_done[label] >= 1):
                    warmups_done[label] += 1
                else:
                    times[label].append(t)
                    cpus[label].append(c)
                    if m is not None:
                        mems[label].append(m)
                    if len(times[label]) >= runs or (over_budget(label) and len(times[label]) >= min_runs):
                        completed.add(label)
            if progress:
                progress(label, counts, cancelled, completed)
        round_idx += 1

    return (
        [(label, times[label]) for label, _ in variants],
        [(label, mems[label]) for label, _ in variants],
        [(label, cpus[label]) for label, _ in variants],
        cancelled,
    )


def print_progress(label, current, total, width=30):
    """Barra de progreso de una sola línea (se sobreescribe con \\r) en stderr."""
    if total <= 0:
        return
    filled = int(width * current / total)
    bar = "#" * filled + "-" * (width - filled)
    pct = 100 * current // total
    end = "\n" if current >= total else ""
    sys.stderr.write(f"\r{label:<20} [{bar}] {current:>3}/{total} {pct:>3}%{end}")
    sys.stderr.flush()


def ordered_names(names):
    """Aplana grouped_names(names) de vuelta a una lista plana — mismo orden que
    se muestra agrupado en list_programs, para que la numeración de resolve()
    coincida con la que ve el usuario en pantalla."""
    return [n for _, group in grouped_names(names) for n in group]


def list_programs(base_dir, names, exclude):
    index_of = {n: i for i, n in enumerate(names, 1)}
    for cat, group_names in grouped_names(names):
        print(f"\n{cat}")
        for n in group_names:
            variants = variants_for(base_dir, n, exclude)
            labels = " ".join(v.split(".", 1)[1] if "." in v else v for v, _ in variants)
            desc = describe(n)
            suffix = f" — {desc}" if desc else ""
            print(f"  {index_of[n]:2d}) {n:<16} {labels}{suffix}")


def resolve(arg, names):
    if arg.isdigit():
        idx = int(arg) - 1
        if idx < 0 or idx >= len(names):
            sys.exit(f"error: número fuera de rango (1..{len(names)})")
        return [names[idx]]
    if arg in names:
        return [arg]
    matches = [n for n in names if arg in n]
    if not matches:
        sys.exit(f"error: ningún programa coincide con '{arg}'")
    return matches


HEADERS = ("Variante", "Mediana", "Mín", "Máx", "MAD", "Ratio")


def _mad(samples, med):
    """Desviación absoluta mediana: dispersión robusta (un outlier no la infla,
    a diferencia de σ)."""
    return statistics.median(abs(s - med) for s in samples)


def build_rows(results, fmt):
    """results: [(etiqueta, muestras), ...]. Ordena por mediana ascendente.

    Mediana y MAD en vez de media y σ: en un workload determinista el ruido es
    siempre aditivo (una pausa del scheduler solo puede sumar), así que un solo
    outlier arrastra la media pero no la mediana. El mínimo queda como el mejor
    estimador del costo sin interferencia."""
    stats = []
    for label, samples in results:
        if not samples:
            stats.append((label, None))
            continue
        med = statistics.median(samples)
        stats.append((label, (med, min(samples), max(samples), _mad(samples, med))))

    ok = [s for s in stats if s[1] is not None]
    failed = [s for s in stats if s[1] is None]
    ok.sort(key=lambda s: s[1][0])

    baseline = ok[0][1][0] if ok else None
    rows = []
    for label, (med, mn, mx, mad) in ok:
        ratio = f"{med / baseline:.2f}x" if baseline else "-"
        rows.append([label, fmt(med), fmt(mn), fmt(mx), fmt(mad), ratio])
    for label, _ in failed:
        rows.append([label, "sin datos", "-", "-", "-", "-"])
    return rows


def quality_warnings(time_results, cpu_results):
    """Avisos de sesión contaminada, para no leer como señal lo que fue ruido:

    - outliers: >10% de las muestras por encima de mediana + max(10·MAD, 1%,
      1 ms). El umbral es deliberadamente tolerante (hyperfine usa ~22·MAD):
      con MAD típicos de 0.3% de la mediana, 3·MAD marcaba la cola derecha
      fisiológica del 1-2% y el aviso perdía valor. Una pausa real de scheduler
      mete +10-200 ms y supera este umbral por órdenes de magnitud igual;
    - interferencia: >10% de las corridas con pared ≫ CPU (ratio >1.5 y
      diferencia >10 ms) — el SO desalojó al proceso, la pared no es del programa.
    """
    warns = []
    cpu_by_label = dict(cpu_results)
    for label, samples in time_results:
        if len(samples) < 4:
            continue
        med = statistics.median(samples)
        threshold = med + max(10 * _mad(samples, med), 0.01 * med, 0.001)
        outliers = sum(1 for s in samples if s > threshold)
        if outliers > len(samples) * 0.1:
            warns.append(f"{label}: {outliers}/{len(samples)} outliers (≫ mediana+10·MAD) — sesión con ruido, conviene repetirla")
        cpus = cpu_by_label.get(label) or []
        preempted = sum(1 for t, c in zip(samples, cpus) if t - c > 0.010 and t > c * 1.5)
        if preempted > len(cpus) * 0.1:
            warns.append(f"{label}: pared ≫ CPU en {preempted}/{len(cpus)} corridas — hubo desalojo del SO durante la medición")
    return warns


def print_table(rows, headers=HEADERS):
    widths = [len(h) for h in headers]
    for r in rows:
        for i, c in enumerate(r):
            widths[i] = max(widths[i], len(c))

    def fmt_row(cols):
        return "  ".join(c.ljust(widths[i]) for i, c in enumerate(cols))

    print(fmt_row(headers))
    print("  ".join("-" * w for w in widths))
    for r in rows:
        print(fmt_row(r))


def write_markdown(path, name, rows, headers=HEADERS, note=None):
    with open(path, "a") as f:
        f.write(f"## {name}\n\n")
        if note:
            f.write(f"_{note}_\n\n")
        f.write("| " + " | ".join(headers) + " |\n")
        f.write("|" + "---|" * len(headers) + "\n")
        for r in rows:
            f.write("| " + " | ".join(r) + " |\n")
        f.write("\n")


RANKING_HEADERS = ("Variante", "Tiempo (x líder)", "Memoria (x líder)", "Score combinado", "Puesto")

RANKING_EXPLANATION = (
    "Score = √(tiempo ÷ mejor_tiempo × memoria ÷ mejor_memoria) — media geométrica "
    "de cuánto peor está cada variante respecto al líder de esa métrica (medianas). "
    "1.00 = una misma variante lidera tiempo Y memoria a la vez; cuanto más alto, peor. "
    "Variantes estadísticamente indistinguibles (ventanas mediana±2·MAD solapadas en "
    "ambas métricas) comparten puesto, marcado con '='."
)


def _overlaps(med_a, mad_a, med_b, mad_b):
    """¿Se solapan las ventanas mediana ± max(2·MAD, 0.5%)? Si sí, la diferencia
    entre ambas variantes es del orden del ruido y no justifica un orden. El piso
    relativo cubre el caso MAD=0 (muestras cuantizadas idénticas): una diferencia
    <1% con dispersión cero sigue sin ser rankeable con esta metodología."""
    half_a = max(2 * mad_a, 0.005 * med_a)
    half_b = max(2 * mad_b, 0.005 * med_b)
    return med_a - half_a <= med_b + half_b and med_b - half_b <= med_a + half_a


def build_ranking_raw(time_results, mem_results):
    """Ranking por score combinado = media geométrica de (tiempo/mejor, memoria/mejor),
    sobre MEDIANAS. Menor score es mejor; 1.00 solo si una misma variante lidera ambas.

    Devuelve [{"label", "time_ratio", "mem_ratio", "score", "rank", "tied"}, ...]
    ordenado ascendente por score. Variantes adyacentes cuyas ventanas mediana±2·MAD
    se solapan en tiempo Y en memoria comparten "rank" (empate técnico, "tied")."""
    time_stats, mem_stats = {}, {}
    for label, s in time_results:
        if s:
            med = statistics.median(s)
            time_stats[label] = (med, _mad(s, med))
    for label, s in mem_results:
        if s:
            med = statistics.median(s)
            mem_stats[label] = (med, _mad(s, med))
    labels = [l for l in time_stats if l in mem_stats]
    if not labels:
        return []

    best_time = min(time_stats[l][0] for l in labels)
    best_mem = min(mem_stats[l][0] for l in labels)

    scored = []
    for l in labels:
        rt = time_stats[l][0] / best_time
        rm = mem_stats[l][0] / best_mem
        scored.append({"label": l, "time_ratio": rt, "mem_ratio": rm, "score": (rt * rm) ** 0.5})
    scored.sort(key=lambda d: d["score"])

    for i, d in enumerate(scored):
        if i == 0:
            d["rank"] = 1
            continue
        prev = scored[i - 1]
        tied = (_overlaps(*time_stats[d["label"]], *time_stats[prev["label"]])
                and _overlaps(*mem_stats[d["label"]], *mem_stats[prev["label"]]))
        d["rank"] = prev["rank"] if tied else i + 1
    for i, d in enumerate(scored):
        d["tied"] = any(o["rank"] == d["rank"] for j, o in enumerate(scored) if j != i)
    return scored


def build_ranking(time_results, mem_results):
    """Como build_ranking_raw, pero como filas de tabla con valores ya formateados."""
    scored = build_ranking_raw(time_results, mem_results)
    rows = []
    for d in scored:
        puesto = f"#{d['rank']}" + ("=" if d["tied"] else "")
        rows.append([d["label"], f"{d['time_ratio']:.2f}x", f"{d['mem_ratio']:.2f}x", f"{d['score']:.2f}", puesto])
    return rows


def write_csv_rows(path, header, rows, meta=None):
    """Agrega filas al CSV. En un archivo nuevo antepone `meta` como líneas de
    comentario '# clave: valor' y luego el header."""
    import csv
    new_file = not os.path.exists(path) or os.path.getsize(path) == 0
    with open(path, "a", newline="") as f:
        if new_file and meta:
            for k, v in meta:
                f.write(f"# {k}: {v}\n")
        w = csv.writer(f)
        if new_file:
            w.writerow(header)
        w.writerows(rows)


# ── Metadatos del entorno ────────────────────────────────────────────────

# Comando de versión por clave de variante (para registrar el entorno exacto).
VERSION_CMDS = {
    "native": ["ray", "--version"],
    "ray": ["ray", "--version"],
    "go": ["go", "version"],
    "rs": ["rustc", "--version"],
    "js": ["node", "--version"],
    "lua": ["lua", "-v"],
    "php": ["php", "--version"],
    "pl": ["perl", "--version"],
    "py": ["python3", "--version"],
    "rb": ["ruby", "--version"],
}


def _version_line(cmd):
    try:
        out = subprocess.run(cmd, capture_output=True, text=True, timeout=10)
        text = (out.stdout or out.stderr).strip()
        for line in text.splitlines():  # perl -v arranca con línea vacía
            if line.strip():
                return line.strip()
    except (OSError, subprocess.TimeoutExpired):
        pass
    return None


def env_metadata(exclude=(), runs=None, warmup=None):
    """Entorno de la corrida como [(clave, valor), ...] para los exports: un
    resultado sin fecha/CPU/versiones no es reproducible ni comparable con la
    corrida del mes que viene."""
    import datetime
    import platform

    meta = [("fecha", datetime.datetime.now().astimezone().isoformat(timespec="seconds"))]
    if sys.platform == "darwin":
        cpu = _version_line(["sysctl", "-n", "machdep.cpu.brand_string"])
        if cpu:
            meta.append(("cpu", cpu))
    meta.append(("os", platform.platform()))
    if runs is not None:
        meta.append(("runs/warmup", f"{runs}/{warmup}"))

    seen = set()
    for key, cmd in VERSION_CMDS.items():
        if key in exclude or cmd[0] in seen:
            continue
        seen.add(cmd[0])
        version = _version_line(cmd)
        if version:
            meta.append((cmd[0], version))
    return meta


def write_markdown_metadata(path, meta):
    """Bloque de entorno al inicio de una sesión de export (append: en un archivo
    existente separa la sesión nueva de la anterior)."""
    with open(path, "a") as f:
        f.write("## Entorno\n\n")
        for k, v in meta:
            f.write(f"- **{k}**: {v}\n")
        f.write("\n")
