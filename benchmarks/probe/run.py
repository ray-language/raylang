#!/usr/bin/env python3
"""Banco de SONDEO (nov 2026): los rasgos del lenguaje que `poly/` no ejercita — clausuras, structs,
enums, traits, strings, mapas, flotantes, Result, bytes, canales, ordenación — en raylang (nativo y
VM), Go y Node. Mediana del tiempo auto-cronometrado + RSS pico; comprueba que las salidas coinciden.

    ./run.py --build            # compila los nativos y los Go (requiere `ray`, `go`)
    ./run.py [programa…]        # mide (RUNS=7 por defecto); RAY=/ruta/a/ray para otro toolchain
    ./run.py --micro            # micro.ray: ns por operación, nativo y VM

No va a CI: es una herramienta de sesión, como `poly/`. Las cifras, en PERFORMANCE.md."""
import os, re, resource, statistics, subprocess, sys, time

RAY = os.environ.get("RAY", "ray")
HERE = os.path.dirname(os.path.abspath(__file__))
RUNS = int(os.environ.get("RUNS", "7"))


def run_once(cmd):
    t0 = time.perf_counter()
    p = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    out, err = p.communicate()
    wall = time.perf_counter() - t0
    m = re.search(rb"bench_ns=(\d+)", err)
    return (int(m.group(1)) if m else None), wall, out, p.returncode, err


def rss_of(cmd):
    r = subprocess.run(["/usr/bin/time", "-l"] + cmd, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    m = re.search(rb"(\d+)\s+maximum resident set size", r.stderr)
    return int(m.group(1)) if m else 0


def programs():
    return sorted(d for d in os.listdir(HERE) if os.path.isdir(os.path.join(HERE, d)) and not d.startswith("_"))


def build():
    for name in programs():
        d = os.path.join(HERE, name)
        subprocess.run([RAY, "build", "--native", "--release", os.path.join(d, name + ".ray"), "-o", os.path.join(d, name)], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        subprocess.run(["go", "build", "-o", os.path.join(d, name + "-go"), os.path.join(d, name + ".go")], check=True)
        print("built", name, flush=True)
    subprocess.run([RAY, "build", "--native", "--release", os.path.join(HERE, "micro.ray"), "-o", os.path.join(HERE, "micro")], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def micro():
    def load(cmd):
        acc = {}
        for _ in range(3):
            out = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL).stdout.decode()
            for line in out.splitlines():
                n, a, b, _c = line.split("\t")
                acc.setdefault(n, []).append(float(a) + float(b) / 10)
        return {k: sorted(v)[1] for k, v in acc.items()}

    nat = load([os.path.join(HERE, "micro")])
    vm = load([RAY, "run", os.path.join(HERE, "micro.ray")])
    print("| op | nativo ns | VM ns | VM÷nativo |\n|---|---|---|---|")
    for k in nat:
        print(f"| {k} | {nat[k]:.1f} | {vm.get(k, 0):.1f} | {vm.get(k, 0) / max(nat[k], 0.05):.0f}× |")


def main():
    if sys.argv[1:2] == ["--build"]:
        return build()
    if sys.argv[1:2] == ["--micro"]:
        return micro()
    names = sys.argv[1:] or programs() or []
    rows = []
    for name in names:
        d = os.path.join(HERE, name)
        variants = {}
        if os.path.exists(os.path.join(d, name + ".ray")):
            variants["native"] = [os.path.join(d, name)]
            variants["vm"] = [RAY, "run", os.path.join(d, name + ".ray")]
        if os.path.exists(os.path.join(d, name + "-go")):
            variants["go"] = [os.path.join(d, name + "-go")]
        if os.path.exists(os.path.join(d, name + ".js")):
            variants["node"] = ["node", os.path.join(d, name + ".js")]
        ref_out = None
        res = {}
        for v, cmd in variants.items():
            if v == "native" and not os.path.exists(cmd[0]):
                continue
            times = []
            bad = None
            for i in range(RUNS + 1):
                ns, wall, out, code, err = run_once(cmd)
                if code != 0:
                    bad = f"exit {code}: {err[-200:]!r}"
                    break
                if i == 0:
                    if ref_out is None:
                        ref_out = out
                    elif out != ref_out:
                        bad = f"OUTPUT DIFFERS: {out[:80]!r} vs {ref_out[:80]!r}"
                    continue  # warmup
                times.append(ns if ns is not None else int(wall * 1e9))
            if bad and not times:
                res[v] = (None, 0, bad)
                continue
            res[v] = (statistics.median(times) / 1e6, rss_of(cmd) / 1e6, bad)
        rows.append((name, res))
        line = f"{name:10s}"
        for v in ["native", "go", "node", "vm"]:
            if v in res:
                ms, mb, bad = res[v]
                line += f" | {v} " + (f"{ms:8.1f} ms {mb:6.1f} MB" if ms is not None else "FAIL") + (f" [{bad}]" if bad else "")
        print(line, flush=True)
    print("\n| probe | native ms | go ms | node ms | vm ms | native/go | native/node | vm/native | native MB | go MB | node MB |")
    print("|---|---|---|---|---|---|---|---|---|---|---|")
    for name, res in rows:
        g = lambda v, i=0: res[v][i] if v in res and res[v][0] is not None else None
        n, go, js, vm = g("native"), g("go"), g("node"), g("vm")
        f = lambda x, fmt="{:.1f}": fmt.format(x) if x is not None else "—"
        r = lambda a, b: "{:.2f}×".format(a / b) if a and b else "—"
        print(f"| {name} | {f(n)} | {f(go)} | {f(js)} | {f(vm)} | {r(n, go)} | {r(n, js)} | {r(vm, n)} | {f(g('native',1))} | {f(g('go',1))} | {f(g('node',1))} |")


main()
