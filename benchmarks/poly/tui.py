#!/usr/bin/env python3
"""
tui — interfaz de texto interactiva para bench.py: selector de programas y
variantes con teclado, corrida con barras de progreso en vivo y gráficos de
barras finales (tiempo, memoria y ranking combinado) dentro de la terminal.

No depende de librerías externas: usa `curses` (stdlib).

Uso:
    ./tui.py

Controles:
    ↑/↓ o j/k   mover selección
    espacio     marcar/desmarcar (listas múltiples)
    enter       confirmar
    a           seleccionar/deseleccionar todos
    +/-         ajustar corridas o calentamiento (pantalla de config)
    q / esc     volver / salir
"""

import curses
import os
import statistics
import textwrap

import benchlib

DIR = os.path.dirname(os.path.abspath(__file__))

BLOCK = "█"
MEDALS = ["🥇", "🥈", "🥉"]


def addstr(stdscr, y, x, text, attr=curses.A_NORMAL):
    """stdscr.addstr, pero silenciosa: curses tira error al escribir en la
    última celda de la pantalla (esquina inferior derecha) o si y/x quedan
    fuera de rango tras un resize; no vale la pena crashear la TUI por eso."""
    try:
        stdscr.addstr(y, x, text, attr)
    except curses.error:
        pass


# ── Widgets de selección ─────────────────────────────────────────────────

class Header:
    """Encabezado de grupo no seleccionable dentro de una select_list (p.ej.
    nombre de categoría). select_list lo salta al navegar con flechas."""
    __slots__ = ("text",)

    def __init__(self, text):
        self.text = text


DESC_COL = 22


def select_list(stdscr, title, items, footer="↑↓ mover · enter elegir · q salir",
                 corner=None, extra_keys=None, descriptions=None):
    """Selector de un solo ítem. Devuelve el índice elegido o None si se cancela.

    `items` puede mezclar strings (seleccionables) y Header (encabezados de
    grupo, se muestran atenuados y la navegación los salta).
    `corner`: texto de panel mostrado alineado a la derecha en la fila del título
    (p.ej. un resumen de configuración). `extra_keys`: dict {carácter: valor} —
    si se presiona esa tecla, select_list retorna ese valor de inmediato en vez
    de navegar la lista (usado para abrir la pantalla de configuración con 's').
    `descriptions`: dict {ítem: texto}, se muestra atenuado a la derecha del ítem.
    """
    extra_keys = extra_keys or {}
    descriptions = descriptions or {}
    selectable = [i for i, it in enumerate(items) if not isinstance(it, Header)]
    if not selectable:
        return None
    pos = 0
    idx = selectable[pos]
    curses.curs_set(0)
    while True:
        stdscr.erase()
        h, w = stdscr.getmaxyx()
        addstr(stdscr, 0, 0, title[: w - 1], curses.A_BOLD)
        if corner:
            text = corner[: max(0, w - 2)]
            addstr(stdscr, 0, max(0, w - len(text) - 1), text, curses.A_DIM)
        for i, it in enumerate(items):
            y = i + 2
            if y >= h - 2:
                break
            if isinstance(it, Header):
                addstr(stdscr, y, 0, it.text[: w - 1], curses.A_DIM)
            else:
                attr = curses.A_REVERSE if i == idx else curses.A_NORMAL
                addstr(stdscr, y, 2, it[: w - 3], attr)
                desc = descriptions.get(it)
                if desc:
                    avail = w - DESC_COL - 1
                    if avail > 3:
                        addstr(stdscr, y, DESC_COL, desc[:avail], curses.A_DIM)
        addstr(stdscr, h - 1, 0, footer[: w - 1], curses.A_DIM)
        stdscr.refresh()

        key = stdscr.getch()
        ch = chr(key) if 0 <= key < 256 else ""
        if ch in extra_keys:
            return extra_keys[ch]
        elif key in (curses.KEY_UP, ord("k")):
            pos = (pos - 1) % len(selectable)
            idx = selectable[pos]
        elif key in (curses.KEY_DOWN, ord("j")):
            pos = (pos + 1) % len(selectable)
            idx = selectable[pos]
        elif key in (curses.KEY_ENTER, 10, 13):
            return idx
        elif key in (ord("q"), 27):
            return None


def select_checkboxes(stdscr, title, items, checked, footer="↑↓ mover · espacio marcar · a todos · enter confirmar · q cancelar"):
    """Selector de checkboxes. `checked` es una lista de bool (misma longitud que items),
    se muta in-place y también se devuelve; None si se cancela."""
    idx = 0
    curses.curs_set(0)
    while True:
        stdscr.erase()
        h, w = stdscr.getmaxyx()
        addstr(stdscr, 0, 0, title[: w - 1], curses.A_BOLD)
        for i, label in enumerate(items):
            y = i + 2
            if y >= h - 2:
                break
            box = "[x]" if checked[i] else "[ ]"
            attr = curses.A_REVERSE if i == idx else curses.A_NORMAL
            addstr(stdscr, y, 2, f"{box} {label}"[: w - 3], attr)
        addstr(stdscr, h - 1, 0, footer[: w - 1], curses.A_DIM)
        stdscr.refresh()

        key = stdscr.getch()
        if key in (curses.KEY_UP, ord("k")):
            idx = (idx - 1) % len(items)
        elif key in (curses.KEY_DOWN, ord("j")):
            idx = (idx + 1) % len(items)
        elif key == ord(" "):
            checked[idx] = not checked[idx]
        elif key == ord("a"):
            all_on = not all(checked)
            checked[:] = [all_on] * len(checked)
        elif key in (curses.KEY_ENTER, 10, 13):
            return checked
        elif key in (ord("q"), 27):
            return None


def select_number(stdscr, title, value, step=1, minimum=1, maximum=1000):
    """Ajusta un número con +/-/flechas. Enter confirma, q/esc cancela (devuelve None)."""
    curses.curs_set(0)
    while True:
        stdscr.erase()
        h, w = stdscr.getmaxyx()
        addstr(stdscr, 0, 0, title[: w - 1], curses.A_BOLD)
        addstr(stdscr, 2, 2, f"Valor: {value}", curses.A_REVERSE)
        addstr(stdscr, h - 1, 0, "+/- o ←→ ajustar · enter confirmar · q cancelar"[: w - 1], curses.A_DIM)
        stdscr.refresh()

        key = stdscr.getch()
        if key in (ord("+"), ord("="), curses.KEY_UP, curses.KEY_RIGHT):
            value = min(maximum, value + step)
        elif key in (ord("-"), curses.KEY_DOWN, curses.KEY_LEFT):
            value = max(minimum, value - step)
        elif key in (curses.KEY_ENTER, 10, 13):
            return value
        elif key in (ord("q"), 27):
            return None


# ── Corrida con progreso en vivo ─────────────────────────────────────────

def confirm_cancel(stdscr, label):
    """Modal de confirmación antes de cancelar una variante. Bloquea (nodelay
    off) hasta que el usuario responda s/n; siempre se restaura nodelay al salir."""
    h, w = stdscr.getmaxyx()
    msg = f"¿Cancelar '{label}'? (s: sí, cualquier otra tecla: no)"
    stdscr.nodelay(False)
    try:
        addstr(stdscr, h - 2, 0, msg[: w - 1], curses.A_REVERSE)
        stdscr.refresh()
        key = stdscr.getch()
        return key in (ord("s"), ord("S"), ord("y"), ord("Y"))
    finally:
        stdscr.nodelay(True)


def run_benchmark(stdscr, names, exclude, runs, warmup, budget_s):
    """Corre todas las variantes de todos los programas, intercaladas por
    rondas (benchlib.run_variants): todas las barras avanzan en paralelo. ESC
    durante una corrida pide confirmación para EXCLUIR la variante en curso de
    las rondas restantes (sus muestras ya medidas se conservan) y las demás
    siguen — no hace falta abortar todo por una sola variante lenta."""
    curses.curs_set(0)
    stdscr.nodelay(True)
    all_results = {}  # name -> (time_results, mem_results, cpu_results)

    try:
        for name in names:
            variants = benchlib.variants_for(DIR, name, exclude)
            if not variants:
                continue
            total = warmup + runs

            # ESC reacciona en ~50ms (measure_once sondea el proceso) sin
            # importar cuánto tarde la corrida individual.
            def check_cancel(label):
                return stdscr.getch() == 27 and confirm_cancel(stdscr, label)

            def progress(label, counts, cancelled, completed, name=name, variants=variants, total=total):
                draw_all_progress(stdscr, name, variants, counts, total, cancelled, label, completed)

            draw_all_progress(stdscr, name, variants, {l: 0 for l, _ in variants}, total, set(), None, set())
            time_results, mem_results, cpu_results, _ = benchlib.run_variants(
                variants, runs, warmup, progress=progress, check_cancel=check_cancel,
                budget_s=budget_s)
            all_results[name] = (time_results, mem_results, cpu_results)
    finally:
        stdscr.nodelay(False)

    return all_results


def draw_all_progress(stdscr, name, variants, counts, total, cancelled, current_label, completed):
    """Una línea por variante, todas avanzando en paralelo (las corridas van
    intercaladas por rondas): la que acaba de correr se resalta, las excluidas
    con ESC quedan marcadas, las completas cierran con 'listo' — anticipado si
    el presupuesto de tiempo las cortó antes de las corridas máximas."""
    stdscr.erase()
    h, w = stdscr.getmaxyx()
    addstr(stdscr, 0, 0, f"Benchmarking: {name} (rondas intercaladas)"[: w - 1], curses.A_BOLD)

    label_w = min(max(len(l) for l, _ in variants) + 3, max(14, w // 2))
    bar_w = max(8, w - label_w - 16)
    done_attr = curses.color_pair(1) if curses.has_colors() else curses.A_NORMAL
    cancelled_attr = curses.color_pair(2) if curses.has_colors() else curses.A_DIM

    y = 2
    for label, _ in variants:
        if y >= h - 2:
            break
        done = counts.get(label, 0)
        if label in cancelled:
            addstr(stdscr, y, 0, label.ljust(label_w)[:label_w], cancelled_attr)
            addstr(stdscr, y, label_w, f"excluida ⨯ ({done}/{total} corridas)", cancelled_attr)
        elif label in completed or done >= total:
            extra = f" ({done}/{total}, presupuesto)" if done < total else ""
            addstr(stdscr, y, 0, label.ljust(label_w)[:label_w])
            addstr(stdscr, y, label_w, BLOCK * bar_w, done_attr)
            addstr(stdscr, y, label_w + bar_w + 1, f"listo ✓{extra}", done_attr)
        else:
            filled = int(bar_w * done / total) if total else bar_w
            bar = BLOCK * filled + "-" * (bar_w - filled)
            pct = 100 * done // total if total else 100
            attr = curses.A_BOLD if label == current_label else curses.A_NORMAL
            addstr(stdscr, y, 0, label.ljust(label_w)[:label_w], attr)
            addstr(stdscr, y, label_w, bar, attr)
            addstr(stdscr, y, label_w + bar_w + 1, f"{done}/{total} {pct}%", attr)
        y += 1

    addstr(stdscr, h - 1, 0, "Corriendo, por favor esperá... (esc: excluir variante actual)"[: w - 1], curses.A_DIM)
    stdscr.refresh()


# ── Gráficos finales ──────────────────────────────────────────────────────

def color_for_rank(i, n):
    if not curses.has_colors():
        return curses.A_NORMAL
    if i == 0:
        return curses.color_pair(1)  # verde: mejor
    if i == n - 1:
        return curses.color_pair(2)  # rojo: peor
    return curses.color_pair(3)  # amarillo: intermedio


def _bar_lines(title, results, fmt):
    """Construye las líneas de un gráfico de barras a partir de [(label, muestras), ...].
    Las variantes sin muestras (canceladas con ESC, o que fallaron) se listan aparte,
    atenuadas, en vez de desaparecer silenciosamente del gráfico."""
    lines = [(title, curses.A_BOLD)]
    means = sorted(((l, statistics.median(s)) for l, s in results if s), key=lambda x: x[1])
    empty = [l for l, s in results if not s]

    if not means and not empty:
        lines.append(("  (sin datos)", curses.A_NORMAL))
        return lines

    if means:
        max_val = means[-1][1]
        for i, (label, val) in enumerate(means):
            lines.append((("BAR", label, max_val, val, fmt(val), color_for_rank(i, len(means))), None))

    for label in empty:
        lines.append((f"  {label} — cancelado / sin datos", curses.A_DIM))

    return lines


def _ranking_lines(scored, w):
    """Barras del ranking combinado, con medalla para el podio (top 3) y el
    detalle de tiempo/memoria de cada variante junto al score."""
    title = "🏆 Ranking combinado (tiempo + memoria) — más corto es mejor"
    lines = [(title, curses.A_BOLD)]
    for wrapped in textwrap.wrap(benchlib.RANKING_EXPLANATION, max(20, w - 2)):
        lines.append((f"  {wrapped}", curses.A_DIM))
    if not scored:
        lines.append(("  (sin datos)", curses.A_NORMAL))
        return lines

    max_val = scored[-1]["score"]
    for i, d in enumerate(scored):
        rank = d.get("rank", i + 1)
        tie = "=" if d.get("tied") else ""
        badge = (MEDALS[rank - 1] if rank <= len(MEDALS) else f"#{rank}") + tie
        label = f"{badge} {d['label']}"
        detail = f"score {d['score']:.2f}  (t {d['time_ratio']:.2f}x · m {d['mem_ratio']:.2f}x)"
        lines.append((("BAR", label, max_val, d["score"], detail, color_for_rank(i, len(scored))), None))
    return lines


def show_results(stdscr, name, time_results, mem_results, cpu_results):
    curses.curs_set(0)
    scroll = 0
    warn_attr = curses.color_pair(3) if curses.has_colors() else curses.A_BOLD
    while True:
        stdscr.erase()
        h, w = stdscr.getmaxyx()

        scored = benchlib.build_ranking_raw(time_results, mem_results)

        content = []
        content += _bar_lines("Tiempo de ejecución (menor es mejor)", time_results, benchlib.human_time)
        content.append(("", curses.A_NORMAL))
        content += _bar_lines("Memoria pico (menor es mejor)", mem_results, benchlib.human_bytes)
        content.append(("", curses.A_NORMAL))
        content += _ranking_lines(scored, w)
        warnings = benchlib.quality_warnings(time_results, cpu_results)
        if warnings:
            content.append(("", curses.A_NORMAL))
            for warning in warnings:
                for wrapped in textwrap.wrap(f"⚠ {warning}", max(20, w - 2)):
                    content.append((wrapped, warn_attr))

        visible_h = h - 3
        max_scroll = max(0, len(content) - visible_h)
        scroll = max(0, min(scroll, max_scroll))

        # Ancho de la columna de nombre: según la variante más larga realmente
        # presente (no un tope fijo arbitrario), + padding para separarla de la
        # barra y absorber el ancho doble de los emojis de medalla.
        bar_labels = [t[1] for t, _ in content if isinstance(t, tuple) and t[0] == "BAR"]
        gap = 3
        label_w = min(max((len(l) for l in bar_labels), default=10) + gap, max(14, w // 2))

        title = f"Resultados: {name}"
        addstr(stdscr, 0, 0, title[: w - 1], curses.A_BOLD | curses.A_UNDERLINE)
        desc = benchlib.describe(name)
        if desc:
            x = len(title) + 2
            avail = w - x - 1
            if avail > 3:
                addstr(stdscr, 0, x, desc[:avail], curses.A_DIM)
        y = 2
        for item in content[scroll: scroll + visible_h]:
            text, attr = item
            if isinstance(text, tuple) and text[0] == "BAR":
                _, label, max_val, val, val_str, bar_attr = text
                bar_max = max(4, w - label_w - len(val_str) - 4)
                filled = int(bar_max * val / max_val) if max_val else 0
                bar = BLOCK * max(1, filled)
                addstr(stdscr, y, 0, label[: label_w - 1])
                addstr(stdscr, y, label_w, bar[: max(0, w - label_w - 1)], bar_attr)
                addstr(stdscr, y, min(w - len(val_str) - 1, label_w + len(bar) + 1), val_str)
            else:
                addstr(stdscr, y, 0, str(text)[: w - 1], attr)
            y += 1

        footer = "↑↓ scroll · enter/q volver al menú"
        addstr(stdscr, h - 1, 0, footer[: w - 1], curses.A_DIM)
        stdscr.refresh()

        key = stdscr.getch()
        if key in (curses.KEY_UP, ord("k")):
            scroll -= 1
        elif key in (curses.KEY_DOWN, ord("j")):
            scroll += 1
        elif key in (curses.KEY_ENTER, 10, 13, ord("q"), 27):
            return


# ── Configuración persistente ─────────────────────────────────────────────

def settings_summary(all_variant_keys, settings):
    enabled = sum(settings["checked"])
    budget = f"{settings['budget_s']}s" if settings["budget_s"] else "∞"
    return (f"[s] config · {settings['runs']}r/{settings['warmup']}w/{budget} · "
            f"{enabled}/{len(all_variant_keys)} lenguajes")


def save_settings(all_variant_keys, settings):
    """Vuelca la configuración actual a settings.toml para que sobreviva al
    cierre de la TUI (y la vean también bench.py / cpu-bench.py / mem-bench.py)."""
    languages = dict(zip(all_variant_keys, settings["checked"]))
    bench = {"runs": settings["runs"], "warmup": settings["warmup"], "budget_s": settings["budget_s"]}
    benchlib.save_config(DIR, languages, bench)


def open_settings(stdscr, all_variant_keys, settings):
    """Submenú de configuración: lenguajes, corridas, calentamiento. No es parte
    del flujo de correr un benchmark — se accede a demanda con 's'. Cada cambio
    confirmado se persiste en settings.toml."""
    while True:
        summary = settings_summary(all_variant_keys, settings)
        items = ["Lenguajes...", "Corridas...", "Calentamiento...", "Presupuesto...", "Volver"]
        idx = select_list(stdscr, "Configuración", items, footer=summary + "  ·  enter elegir · q volver")
        if idx is None or items[idx] == "Volver":
            return

        if items[idx] == "Lenguajes...":
            result = select_checkboxes(
                stdscr,
                "Variantes a incluir (espacio: marcar, a: todas)",
                all_variant_keys,
                settings["checked"][:],
            )
            if result is not None:
                settings["checked"] = result
                save_settings(all_variant_keys, settings)
        elif items[idx] == "Corridas...":
            r = select_number(stdscr, "Corridas medidas por variante", settings["runs"], minimum=1, maximum=500)
            if r is not None:
                settings["runs"] = r
                save_settings(all_variant_keys, settings)
        elif items[idx] == "Calentamiento...":
            wu = select_number(stdscr, "Corridas de calentamiento", settings["warmup"], minimum=0, maximum=200)
            if wu is not None:
                settings["warmup"] = wu
                save_settings(all_variant_keys, settings)
        elif items[idx] == "Presupuesto...":
            b = select_number(stdscr, "Presupuesto de pared por variante en segundos (0 = sin límite)",
                              settings["budget_s"], minimum=0, maximum=120)
            if b is not None:
                settings["budget_s"] = b
                save_settings(all_variant_keys, settings)


# ── Flujo principal ──────────────────────────────────────────────────────

def main(stdscr):
    if curses.has_colors():
        curses.start_color()
        curses.use_default_colors()
        curses.init_pair(1, curses.COLOR_GREEN, -1)
        curses.init_pair(2, curses.COLOR_RED, -1)
        curses.init_pair(3, curses.COLOR_YELLOW, -1)

    stdscr.keypad(True)

    names = benchlib.ordered_names(benchlib.discover_names(DIR))
    base_exclude = benchlib.resolve_exclude(DIR, "")
    defaults = benchlib.load_bench_defaults(DIR)
    all_variant_keys = sorted(set(benchlib.RUNNERS) | {"native", "go", "rs"})

    settings = {
        "runs": defaults["runs"],
        "warmup": defaults["warmup"],
        "budget_s": defaults["budget_s"],
        "checked": [k not in base_exclude for k in all_variant_keys],
    }

    while True:
        program_items = []
        for cat, group_names in benchlib.grouped_names(names):
            program_items.append(Header(cat))
            program_items.extend(group_names)
        program_items += [Header("Acciones"), "[ejecutar todos]", "[salir]"]

        idx = select_list(
            stdscr,
            "raylang bench — elegí un programa",
            program_items,
            footer="↑↓ mover · enter elegir · s configuración · q salir",
            corner=settings_summary(all_variant_keys, settings),
            extra_keys={"s": "SETTINGS"},
            descriptions={n: benchlib.describe(n) for n in names},
        )

        if idx == "SETTINGS":
            open_settings(stdscr, all_variant_keys, settings)
            continue
        if idx is None or program_items[idx] == "[salir]":
            return

        selected_names = names if program_items[idx] == "[ejecutar todos]" else [program_items[idx]]
        run_exclude = {k for k, on in zip(all_variant_keys, settings["checked"]) if not on}

        results = run_benchmark(stdscr, selected_names, run_exclude, settings["runs"], settings["warmup"],
                                settings["budget_s"])
        for name, (time_results, mem_results, cpu_results) in results.items():
            show_results(stdscr, name, time_results, mem_results, cpu_results)


if __name__ == "__main__":
    curses.wrapper(main)
