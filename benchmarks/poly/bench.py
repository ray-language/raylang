#!/usr/bin/env python3
"""
bench — benchmark poliglota combinado: tiempo de ejecución y memoria pico en
un solo set de corridas (cada ejecución se cronometra Y se mide su memoria a
la vez, con benchlib.measure_once), mostrando ambas tablas una tras otra.

Cada programa vive en su propio directorio (empty/, factorial/, ...) con una
variante por lenguaje (nombre.js, nombre.py, nombre.ray, nombre.go -> binario
nombre-go, nombre.rs -> binario nombre-rs, binario nativo nombre, ...).

Uso:
    ./bench.py                  Lista los programas y pregunta cuál medir
    ./bench.py list             Solo lista los programas numerados
    ./bench.py <n>              Mide el programa número <n> del listado
    ./bench.py <nombre>         Mide todas las variantes de <nombre>
    ./bench.py all              Mide todos los programas

Flags:
    --runs N            corridas por variante (default: 20)
    --warmup N          corridas de calentamiento descartadas (default: 10)
    --prepare CMD       comando de preparación antes de cada corrida (p.ej. "sync")
    --clean             atajo de --prepare "sync"
    --export-md FILE    exporta ambas tablas a Markdown
    --export-csv FILE   exporta los datos crudos (tiempo y memoria) a CSV
    --exclude "a b c"   variantes a excluir (p.ej. "native pl php")

settings.toml, si existe en este directorio, activa/desactiva variantes de
forma persistente (native/go/js/lua/php/pl/py/ray/rb/rs = true|false); se
combina con --exclude.
"""

import argparse
import os
import sys

import benchlib

DIR = os.path.dirname(os.path.abspath(__file__))


def bench(names, runs, warmup, budget_s, prepare, exclude, export_md, export_csv):
    meta = benchlib.env_metadata(exclude, runs, warmup) if (export_md or export_csv) else None
    if export_md:
        benchlib.write_markdown_metadata(export_md, meta)

    for name in names:
        variants = benchlib.variants_for(DIR, name, exclude)
        if not variants:
            print(f"error: nada que medir para '{name}'", file=sys.stderr)
            continue

        print(f">> Midiendo tiempo y memoria de '{name}' ({runs} corridas, {warmup} de calentamiento, intercalado)...",
              file=sys.stderr)
        total = (warmup + runs) * len(variants)
        progress = lambda label, counts, cancelled, completed: benchlib.print_progress(name, sum(counts.values()), total)
        time_results, mem_results, cpu_results, _ = benchlib.run_variants(
            variants, runs, warmup, prepare=prepare, progress=progress, budget_s=budget_s)

        benchlib.print_progress(name, total, total)

        csv_rows = []
        for (label, times), (_, mems) in zip(time_results, mem_results):
            for i, t in enumerate(times):
                mem_val = mems[i] if i < len(mems) else ""
                csv_rows.append([name, label, i, t, mem_val])

        print(f"\n=== {name} (tiempo de ejecución) ===")
        time_rows = benchlib.build_rows(time_results, benchlib.human_time, expected_runs=runs)
        benchlib.print_table(time_rows, headers=benchlib.HEADERS_N)

        print(f"\n=== {name} (memoria pico) ===")
        mem_rows = benchlib.build_rows(mem_results, benchlib.human_bytes, expected_runs=runs)
        benchlib.print_table(mem_rows, headers=benchlib.HEADERS_N)

        print(f"\n=== {name} (ranking combinado: tiempo + memoria) ===")
        print(benchlib.RANKING_EXPLANATION)
        ranking_rows = benchlib.build_ranking(time_results, mem_results)
        benchlib.print_table(ranking_rows, headers=benchlib.RANKING_HEADERS)

        for warning in benchlib.budget_warnings(time_results, runs):
            print(f"⚠ {warning}")
        for warning in benchlib.quality_warnings(time_results, cpu_results):
            print(f"⚠ {warning}")
        print()

        if export_md:
            benchlib.write_markdown(export_md, f"{name} — tiempo de ejecución", time_rows, headers=benchlib.HEADERS_N)
            benchlib.write_markdown(export_md, f"{name} — memoria pico", mem_rows, headers=benchlib.HEADERS_N)
            benchlib.write_markdown(export_md, f"{name} — ranking combinado", ranking_rows,
                                     headers=benchlib.RANKING_HEADERS, note=benchlib.RANKING_EXPLANATION)
        if export_csv:
            benchlib.write_csv_rows(export_csv, ["programa", "variante", "corrida", "segundos", "bytes"],
                                    csv_rows, meta=meta)

    if export_md:
        print(f">> Markdown escrito en: {export_md}", file=sys.stderr)
    if export_csv:
        print(f">> CSV escrito en: {export_csv}", file=sys.stderr)


def main():
    bench_defaults = benchlib.load_bench_defaults(DIR)

    ap = argparse.ArgumentParser(add_help=True)
    ap.add_argument("cmd", nargs="?", default="")
    ap.add_argument("--runs", type=int, default=bench_defaults["runs"])
    ap.add_argument("--warmup", type=int, default=bench_defaults["warmup"])
    ap.add_argument("--budget", type=float, default=bench_defaults["budget_s"],
                    help="presupuesto de pared por variante en segundos (0 = sin límite)")
    ap.add_argument("--prepare", default="")
    ap.add_argument("--clean", action="store_true", help='atajo de --prepare "sync"')
    ap.add_argument("--export-md")
    ap.add_argument("--export-csv")
    ap.add_argument("--exclude", default="")
    args = ap.parse_args()

    prepare = "sync" if args.clean else args.prepare
    exclude = benchlib.resolve_exclude(DIR, args.exclude)
    names = benchlib.ordered_names(benchlib.discover_names(DIR))

    if args.cmd in ("", "choose"):
        benchlib.list_programs(DIR, names, exclude)
        print()
        try:
            sel = input("Elige un número o nombre (o Enter para salir): ").strip()
        except EOFError:
            sel = ""
        if not sel:
            return
        bench(benchlib.resolve(sel, names), args.runs, args.warmup, args.budget, prepare, exclude, args.export_md, args.export_csv)
    elif args.cmd in ("list", "ls"):
        benchlib.list_programs(DIR, names, exclude)
    elif args.cmd == "all":
        bench(names, args.runs, args.warmup, args.budget, prepare, exclude, args.export_md, args.export_csv)
    else:
        bench(benchlib.resolve(args.cmd, names), args.runs, args.warmup, args.budget, prepare, exclude, args.export_md, args.export_csv)


if __name__ == "__main__":
    main()
