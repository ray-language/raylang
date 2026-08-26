// El editor REAL del playground (IDEAS §74): CodeMirror 6 empaquetado con esbuild en un solo
// archivo (`../editor.bundle.js`, commiteado como `raylang.wasm`). Expone una fachada mínima
// (`window.RayEditor`) con lo que el playground necesita: crear el editor con el lenguaje y el
// tema de marca, autocompletado/diagnósticos/hover conectables al LSP (que corre DENTRO del
// wasm de raylang: `src/wasm.rs::lsp`), y helpers de posiciones LSP (línea/columna) ↔ offsets.

import { EditorState } from "@codemirror/state";
import {
    EditorView, keymap, lineNumbers, drawSelection, highlightActiveLine,
    highlightActiveLineGutter, hoverTooltip,
} from "@codemirror/view";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import {
    StreamLanguage, syntaxHighlighting, HighlightStyle, indentUnit, bracketMatching,
} from "@codemirror/language";
import {
    autocompletion, closeBrackets, closeBracketsKeymap, completionKeymap, snippet,
} from "@codemirror/autocomplete";
import { setDiagnostics, lintGutter } from "@codemirror/lint";
import { tags as t } from "@lezer/highlight";

// --- El lenguaje: tokenizador por stream (las mismas clases del viejo overlay, ahora CM6) ---
const KW = new Set("fn let var const if else while for in match return struct enum trait impl import from as pub dyn extern spawn self Self".split(" "));
const TY = new Set("int float bool string char bytes ptr unit Option Result Channel Map".split(" "));
const LIT = new Set(["true", "false"]);

const rayLanguage = StreamLanguage.define({
    name: "raylang",
    startState: () => ({ backtick: false }),
    token(stream, state) {
        if (state.backtick) {
            while (!stream.eol()) {
                if (stream.next() === "`") { state.backtick = false; return "string"; }
            }
            return "string";
        }
        if (stream.match("//")) { stream.skipToEnd(); return "comment"; }
        if (stream.peek() === "`") { stream.next(); state.backtick = true; return "string"; }
        if (stream.match(/^b?"/)) {
            let escaped = false;
            while (!stream.eol()) {
                const c = stream.next();
                if (escaped) { escaped = false; continue; }
                if (c === "\\") { escaped = true; continue; }
                if (c === '"') { break; }
            }
            return "string";
        }
        if (stream.match(/^'(\\.|[^'])'/)) { return "string"; }
        if (stream.match(/^@[A-Za-z_][A-Za-z0-9_]*/)) { return "meta"; }
        if (stream.match(/^[0-9][0-9._eExXa-fA-F]*/)) { return "number"; }
        if (stream.match(/^[A-Za-z_][A-Za-z0-9_]*/)) {
            const w = stream.current();
            if (KW.has(w)) { return "keyword"; }
            if (LIT.has(w)) { return "atom"; }
            if (TY.has(w) || /^[A-Z]/.test(w)) { return "typeName"; }
            if (stream.match(/^\s*\(/, false)) { return "fnName"; }
            return "variableName";
        }
        stream.next();
        return null;
    },
    languageData: { commentTokens: { line: "//" } },
    tokenTable: { fnName: t.function(t.variableName) },
});

// --- Tema de marca (paleta océano sobre el panel navy del playground) ---
const rayTheme = EditorView.theme({
    "&": { backgroundColor: "#0e1c2e", color: "#e6edf3", height: "100%", fontSize: "13px" },
    ".cm-scroller": { fontFamily: '"JetBrains Mono","SF Mono",ui-monospace,Menlo,monospace', lineHeight: "1.55" },
    ".cm-content": { caretColor: "#e6edf3", paddingTop: "10px" },
    ".cm-cursor": { borderLeftColor: "#e6edf3" },
    "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, ::selection": { backgroundColor: "rgba(43,124,224,.30)" },
    ".cm-gutters": { backgroundColor: "#0a131f", color: "#5b7089", border: "none" },
    ".cm-activeLine": { backgroundColor: "rgba(43,124,224,.07)" },
    ".cm-activeLineGutter": { backgroundColor: "rgba(43,124,224,.12)", color: "#8fa6c4" },
    ".cm-matchingBracket": { backgroundColor: "rgba(43,124,224,.25)", outline: "none" },
    ".cm-tooltip": { backgroundColor: "#0a131f", color: "#e6edf3", border: "1px solid #1e3454", borderRadius: "8px" },
    ".cm-tooltip.cm-tooltip-autocomplete > ul": { fontFamily: '"JetBrains Mono",monospace', fontSize: "12.5px" },
    ".cm-tooltip.cm-tooltip-autocomplete > ul > li[aria-selected]": { backgroundColor: "rgba(43,124,224,.28)", color: "#ffffff" },
    ".cm-tooltip.cm-tooltip-hover": { padding: "6px 10px", maxWidth: "34rem", whiteSpace: "pre-wrap", fontFamily: '"JetBrains Mono",monospace', fontSize: "12.5px" },
    ".cm-completionDetail": { color: "#8fa6c4", fontStyle: "normal" },
    ".cm-lintRange-error": { textDecoration: "underline wavy #ff6b6b" },
    ".cm-diagnostic": { fontFamily: '"JetBrains Mono",monospace', fontSize: "12.5px" },
}, { dark: true });

const rayHighlight = HighlightStyle.define([
    { tag: t.keyword, color: "#c9a3ff" },
    { tag: t.typeName, color: "#5bacf7" },
    { tag: t.string, color: "#7ee787" },
    { tag: t.number, color: "#f0a868" },
    { tag: t.atom, color: "#f0a868" },
    { tag: t.comment, color: "#6b7f99", fontStyle: "italic" },
    { tag: t.meta, color: "#e3b341" },
    { tag: t.function(t.variableName), color: "#79c0ff" },
]);

// --- La fachada del playground ---
window.RayEditor = {
    /// Crea el editor. `opts`: { doc, onChange(text), completion(ctx) -> resultado CM o null,
    /// hover(view, pos) -> {pos, end, text} o null, run() }.
    create(parent, opts) {
        const extensions = [
            lineNumbers(),
            highlightActiveLineGutter(),
            highlightActiveLine(),
            history(),
            drawSelection(),
            indentUnit.of("    "),
            bracketMatching(),
            closeBrackets(),
            rayLanguage,
            syntaxHighlighting(rayHighlight),
            rayTheme,
            lintGutter(),
            autocompletion({ override: opts.completion ? [opts.completion] : [] }),
            keymap.of([
                { key: "Mod-Enter", run: () => { if (opts.run) { opts.run(); } return true; } },
                ...closeBracketsKeymap,
                ...completionKeymap,
                ...defaultKeymap,
                ...historyKeymap,
                indentWithTab,
            ]),
            EditorView.updateListener.of((u) => {
                if (u.docChanged && opts.onChange) { opts.onChange(u.state.doc.toString()); }
            }),
        ];
        if (opts.hover) {
            extensions.push(hoverTooltip((view, pos) => {
                const r = opts.hover(view, pos);
                if (!r) { return null; }
                return {
                    pos: r.pos,
                    end: r.end,
                    create: () => {
                        const dom = document.createElement("div");
                        dom.textContent = r.text;
                        return { dom };
                    },
                };
            }));
        }
        return new EditorView({ state: EditorState.create({ doc: opts.doc || "", extensions }), parent });
    },
    getDoc(view) { return view.state.doc.toString(); },
    setDoc(view, text) {
        view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: text } });
    },
    /// Diagnósticos CM ({from, to, severity, message}) — el puente desde publishDiagnostics.
    setDiagnostics(view, diags) { view.dispatch(setDiagnostics(view.state, diags)); },
    /// (línea, carácter) 0-based del LSP → offset del documento (con recortes defensivos).
    posToOffset(view, line, character) {
        const doc = view.state.doc;
        const l = doc.line(Math.max(1, Math.min(line + 1, doc.lines)));
        return Math.min(l.from + Math.max(0, character), l.to);
    },
    offsetToPos(view, offset) {
        const l = view.state.doc.lineAt(offset);
        return { line: l.number - 1, character: offset - l.from };
    },
    /// Aplica un snippet estilo LSP (`${1:x}`, `$0` ya convertido) como `apply` de completion.
    snippet,
};
