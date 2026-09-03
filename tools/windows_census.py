#!/usr/bin/env python3
"""Censo de portabilidad de los ejemplos (M167, docs/windows.md).

Corre cada ejemplo con `main` sobre la VM y registra código de salida, stdout y la cabeza de
stderr; `compare` cruza dos registros (Linux = referencia, Windows = candidato) y clasifica cada
ejemplo. Es INFORMATIVO: nunca falla el job; su salida es la tabla del documento de deudas.

  windows_census.py record  --ray <bin> --out linux.json [--dirs basics,data] [--timeout 60]
  windows_census.py compare --expected linux.json --actual windows.json [--summary out.md]
"""
import argparse, json, os, subprocess, sys, time

# La consola de Windows puede no ser UTF-8 (cp1252): un `→` en un print tumbó el primer censo.
# Es el hueco 5.3 de docs/windows.md, demostrado por el propio censo.
if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EXAMPLES = os.path.join(ROOT, "examples")


def find_examples(dirs):
    out = []
    for d, _subdirs, files in os.walk(EXAMPLES):
        rel_dir = os.path.relpath(d, EXAMPLES).replace(os.sep, "/")
        top = rel_dir.split("/")[0]
        if dirs and top not in dirs:
            continue
        for f in sorted(files):
            if not f.endswith(".ray"):
                continue
            p = os.path.join(d, f)
            with open(p, encoding="utf-8", errors="replace") as fh:
                src = fh.read()
            if "fn main(" not in src:
                continue  # módulo-librería: se cubre vía el ejemplo que lo importa
            out.append(p)
    return sorted(out)


def run_one(ray, path, timeout):
    d = os.path.dirname(path)
    # Con manifiesto, el ejemplo es un proyecto: cwd propio y entrada relativa (imports/deps).
    if os.path.exists(os.path.join(d, "ray.toml")):
        cwd, arg = d, os.path.basename(path)
    else:
        cwd, arg = ROOT, os.path.relpath(path, ROOT)
    t0 = time.time()
    try:
        p = subprocess.run([ray, "run", arg], cwd=cwd, stdin=subprocess.DEVNULL,
                           capture_output=True, timeout=timeout)
        return {"code": p.returncode, "stdout": p.stdout.decode("utf-8", "replace"),
                "stderr": "\n".join(p.stderr.decode("utf-8", "replace").splitlines()[:4]),
                "timeout": False, "secs": round(time.time() - t0, 1)}
    except subprocess.TimeoutExpired as e:
        return {"code": None, "stdout": (e.stdout or b"").decode("utf-8", "replace"),
                "stderr": "\n".join((e.stderr or b"").decode("utf-8", "replace").splitlines()[:4]),
                "timeout": True, "secs": timeout}


def record(args):
    dirs = set(args.dirs.split(",")) if args.dirs else None
    # Ruta ABSOLUTA: los ejemplos con manifiesto corren con cwd propio (la relativa no resolvería).
    args.ray = os.path.abspath(args.ray)
    results = {}
    for p in find_examples(dirs):
        rel = os.path.relpath(p, ROOT).replace(os.sep, "/")
        r = run_one(args.ray, p, args.timeout)
        results[rel] = r
        tag = "TIMEOUT" if r["timeout"] else f"code={r['code']}"
        print(f"{rel}: {tag} ({r['secs']}s)", flush=True)
    with open(args.out, "w", encoding="utf-8") as fh:
        json.dump({"platform": sys.platform, "results": results}, fh, indent=1, sort_keys=True)
    print(f"{len(results)} ejemplos -> {args.out}")


def classify(exp, act):
    """Compara Windows (act) contra Linux (exp). Un `main` que devuelve un entero distinto de
    cero NO es un fallo: lo que cuenta es que AMBAS plataformas coincidan en código y stdout."""
    if exp is None:
        return "SOLO-WINDOWS", ""
    if act is None:
        return "SIN-DATO", ""
    if exp["timeout"] and act["timeout"]:
        return "INTERACTIVO", "ambos exceden el plazo (servidor/TUI): validar a mano"
    if act["timeout"]:
        return "CUELGA-WIN", "solo Windows excede el plazo"
    if exp["timeout"]:
        return "CUELGA-LINUX", "solo Linux excede el plazo (espera al exterior)"
    if exp["code"] != act["code"]:
        head = act["stderr"].strip().splitlines()
        return "CODIGO-DISTINTO", f"linux exit {exp['code']}, windows exit {act['code']}" + (f": {head[0]}" if head else "")
    if exp["stdout"].replace("\r\n", "\n") != act["stdout"].replace("\r\n", "\n"):
        return "DIFIERE", "mismo código de salida, stdout distinto"
    if exp["stdout"] != act["stdout"]:
        return "OK-CRLF", "igual salvo CRLF en stdout"
    return "OK", ""


ORDER = ["CODIGO-DISTINTO", "CUELGA-WIN", "DIFIERE", "OK-CRLF", "CUELGA-LINUX", "INTERACTIVO",
         "SOLO-WINDOWS", "SIN-DATO", "OK"]


def compare(args):
    exp = json.load(open(args.expected, encoding="utf-8"))["results"]
    act = json.load(open(args.actual, encoding="utf-8"))["results"]
    rows = []
    for rel in sorted(set(exp) | set(act)):
        status, note = classify(exp.get(rel), act.get(rel))
        rows.append((rel, status, note))
    counts = {}
    for _, s, _ in rows:
        counts[s] = counts.get(s, 0) + 1
    lines = ["# Censo Windows de los ejemplos", "",
             "| Estado | Ejemplos |", "|---|---|"]
    lines += [f"| {s} | {counts[s]} |" for s in ORDER if s in counts]
    lines += ["", "| Ejemplo | Estado | Detalle |", "|---|---|---|"]
    rows.sort(key=lambda r: (ORDER.index(r[1]), r[0]))
    for rel, s, note in rows:
        if s == "OK":
            continue
        safe_note = note.replace("|", "\\|")
        lines.append(f"| `{rel}` | {s} | {safe_note} |")
    lines += ["", f"OK: {counts.get('OK', 0)} de {len(rows)}"]
    text = "\n".join(lines) + "\n"
    print(text)
    if args.summary:
        with open(args.summary, "w", encoding="utf-8") as fh:
            fh.write(text)


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("record")
    r.add_argument("--ray", required=True)
    r.add_argument("--out", required=True)
    r.add_argument("--dirs", default="")
    r.add_argument("--timeout", type=int, default=60)
    r.set_defaults(fn=record)
    c = sub.add_parser("compare")
    c.add_argument("--expected", required=True)
    c.add_argument("--actual", required=True)
    c.add_argument("--summary", default="")
    c.set_defaults(fn=compare)
    a = ap.parse_args()
    a.fn(a)


if __name__ == "__main__":
    main()
