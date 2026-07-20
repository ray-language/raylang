#!/usr/bin/env python3
# Arqueo completo de identificadores en español/spanglish (M-naming).
# Más amplio que tests/naming_policy.rs: cubre fn/let/var Y struct/enum/trait/const/static,
# campos de struct y parámetros; detecta con (a) la wordlist curada del repo + expansión,
# y (b) heurística de diccionario: token no-inglés (web2) ni jerga conocida → sospechoso.
import os, re, sys, json
from collections import defaultdict

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DIRS = ["src", "tests", "selfhost", "packages", "benchmarks"]

# --- detectores ---
wordlist = set()
with open(f"{ROOT}/tests/naming_policy_es.txt") as f:
    for l in f:
        l = l.strip()
        if l: wordlist.add(l)

# Expansión: palabras españolas comunes en código que la lista curada podría no tener.
expansion = set("""
archivo linea columna cadena cuerpo campo campos ambito ambitos entrada salida valor valores
resultado fichero busca buscar crear crea borrar borra leer lee escribir escribe abrir abre
cerrar cierra tamano tamanio longitud anchura altura izquierda derecha arriba abajo dentro fuera
nombre nombres tipo tipos clase objeto objetos lista listas cola pila arbol nodo nodos hoja
llave llaves clave claves indice indices cuenta contador numero numeros letra letras palabra palabras
frase texto mensaje mensajes error errores fallo fallos exito prueba pruebas caso casos
inicio final comienzo termino paso pasos vuelta vueltas ronda rondas ciclo ciclos bucle bucles
suma resta multiplica divide producto cociente resto division modulo potencia raiz
mayor menor igual distinto verdadero falso cierto vacio vacia lleno llena nuevo nueva viejo vieja
primero primera segundo segunda tercero ultima ultimo anterior siguiente actual previo previa
padre madre hijo hija hijos hermano hermana abuelo nieto
espera esperar espera dormir despertar corre correr ejecuta ejecutar lanza lanzar
envia enviar recibe recibir manda mandar entrega entregar
guarda guardar carga cargar copia copiar mueve mover quita quitar saca sacar mete meter pone poner
agrega agregar anade anadir inserta insertar elimina eliminar
cambia cambiar reemplaza reemplazar sustituye sustituir convierte convertir traduce traducir
verifica verificar valida validar comprueba comprobar revisa revisar chequea chequear
calcula calcular procesa procesar analiza analizar genera generar produce producir construye construir
arma armar monta montar desmonta prepara preparar inicializa inicializar arranca arrancar
detiene detener para parar termina terminar acaba acabar finaliza finalizar
usuario usuarios sesion sesiones peticion peticiones respuesta respuestas conexion conexiones
servidor servidores cliente clientes puerto puertos direccion direcciones ruta rutas camino caminos
carpeta carpetas directorio directorios paquete paquetes modulo modulos funcion funciones
variable variables constante constantes parametro parametros argumento argumentos
resultado resultados retorno devuelve devolver regresa regresar
tabla tablas fila filas columna columnas celda celdas registro registros
fecha fechas hora horas dia dias mes meses ano anos anio anios semana semanas
precio precios costo costos cantidad cantidades total totales parcial subtotal
banco cuenta cuentas saldo saldos deuda deudas pago pagos cobro cobros
figura figuras circulo cuadrado triangulo rectangulo punto puntos
color colores rojo verde azul amarillo negro blanco gris
perro gato animal animales persona personas gente ciudad ciudades pais paises mundo
casa casas puerta puertas ventana ventanas mesa mesas silla sillas
libro libros pagina paginas capitulo capitulos titulo titulos autor autores
juego juegos jugador jugadores turno turnos tablero dado dados carta cartas
temporal temporales auxiliar auxiliares ayudante apoyo soporte
oraculo oraculos espejo espejos banda bandas hueco huecos tramo tramos trozo trozos pedazo
etiqueta etiquetas marca marcas senal senales bandera banderas
estado estados fase fases etapa etapas nivel niveles grado grados
""".split())
# Curados a mano de la pasada anterior (tokens claramente españoles del cubo "sospechoso").
curados = set("""
por como los transpila plazo veces caja trabajo forma sonido pista listo emisor nota profundo pasa
traza sube sirviendo saluda salto secuencia seccion selecciona restante reinicia reenvio rechazos
recolectar repetido repetida puente publico probar privada pregunta prefijo prefijos plano pausa
opuesto objetivo nucleo negocia negar nada moneda mostrable lleva llega llamar llamador lexico
lexicos lexa inyecta invalido internos intento inmutable inferencia inferible indeterminado
inalcanzable importado implementa iguales ignoran hermanas heredado habia generado formatea formado
fila1 fila2 falta existe evita estricto esperando emite elige edad donde documenta diagnostica
detalle detecta detectado determinista deterministas describir definicion decodifica declaracion
cubre cubiertos contenido consumidor conjunto conectar concurrencia concreto componen compila
comparaciones comodin clona clon clonar cierre capturado cancela cambios cambio cae basura basico
basicos avisa asignar asignacion ambiguo acumula activos abierta verificacion verano variedad usos
usan usados une unidas unarios ubicacion tuvo tupla tuplas truncada tres trae tomar tira tipan
tipada terminados tengo temprana tambien superficie sueltos soportado sobreviven sistema sirve
sintetico sintactico simbolo siendo serie serializa senalar semantica semantico seis seguro
secciones sangria saneo saltados reutilizada reusan reusa retorna resuelven respetan resoluciones
resolucion reservado resalta requisitos requisito requiere requerido reposiciona reporte reparsea
rendimiento remoto relanzar reintenta regresion regla regeneran regen reexportados reescribe
redireccion redefinido recursos recursiva recuperado recupera recorrido recorre recoger recibido
reasignar reabrir razonable raro rapida rangos quitado quiere queda puntuacion primitivos preservan
presenta pendientes pelo pasar pasada parentesis origen orientacion ordenado ordenadas operandos
operador opaco omite oculto octetos ociosa obsoletos observabilidad observa obligatorio obligatoria
nunca nuevos nuevas nombra ningun negativo mutar multiplicacion multilinea muchos mostrar minimo
miembro miden mezclada metadatos membresia medible matematicas manipulado manipulada manipulacion
manglada malicioso malformado logicos llevan llamada limpia limita libre liberas libera legadas leen
iterativo invierte invierno inversa invariante invalida intervalos interrogacion intermediario
inteligencia instante instancias inmediato inicial infinito indirecto indica indexacion
independientes indenta incorporados incorporado inconsistente incluye importar importados importada
importa ignora identicos hostil homonimos homonimo homogeneo hijas heredan hereda hashea hacer
guardas guardado grita grande gotcha fuga fuerza fue flujo firmado fibras faltantes fallar fachada
extendido expuesta expresiones expiradas existen exige excluye excluido exacta este estaticos
estatico estable esta esquema escapa escalares ergonomia equivocado envolver envoltorios enviados
enviado entorno entero enruta empates embebida elegir ejercitar ejemplo ejemplos eje ediciones
duplicado distintos distinta disponibles directa direccionable digito diez diccionarios diccionario
dibujar despues desescapa desempaqueta desconocida descomprime descargar descarga desactualizados
derivado deja definidas definida definiciones deduplica declarado declarada declaraciones declara
cumulatividad cuelga cubrir cuantos cuando cuadruple cuadrados cruzan cripto credito correcto
correctas corromper cortocircuito cosa conserva conocidos configurado configurable condicion
compuestos compresion comportamiento comparten compania colapsa coincidir codificados clasificar
clasificados cinco chau causa cascada capturas capturan capitaliza capacidades capacidad
cancelacion cambian calificadas cajas caido caida brazos borrados borde booleanos bloques bloquea
bloqueado bitops binaria bateria basadas baja azucar audita atribuye asociatividad asociadas
asegurar aritmetico apaga anonimas anidadas ancount alterado alta alguno alcanzado ahora adoptada
adopta admite acota acapara absolutizar transitivo transitiva transitivas transaccion traduce
sasl saslname repro construccion estructurado idempotente histograma filtro desemp
protocolo produccion problemas propias propios propio propiedades propagan posicion portada piezas
pierde persisten persiste pero permitido perezosos ofensor ocurrencias ofrecibles
""".split())
es_words = wordlist | expansion | curados

# Diccionario inglés del sistema + jerga técnica/proyecto aceptada (para la heurística b).
english = set()
try:
    with open("/usr/share/dict/words") as f:
        for l in f: english.add(l.strip().lower())
except FileNotFoundError:
    pass
jargon = set("""
fn impl struct enum bool str usize isize i64 u64 u32 u8 f64 vec vecs deque hashmap hashset rc refcell arc mutex
idx ptr len cap init args argv argc env ok err eof ast vm gc lsp cli repl json toml csv http https tcp udp tls
url uri utf ascii regex fmt io fs os ffi rpc html css js sql db id ids uuid jwt hmac sha ed25519 chacha rsa
api abi ip ipv4 ipv6 dns ntp ws wss h2 hpack grpc bson smtp mime
src dst tmp buf bufs pos min max abs sqrt pow mod div mul sub neg cmp eq ne lt le gt ge
elem elems expr exprs stmt stmts tok toks ident idents params param arg lhs rhs op ops
ch chs ck cb ctx cfg cnt col cols ln num nums ns
recv send spawn join fut async sync mpsc
substr prefix suffix concat iter next prev cur
sig sigs impls defs refs decl decls dyn
usr pwd auth login logout
xs ys zs acc res ret val vals kv
mut pub priv
todo fixme xxx
utc iso ms ns us secs millis nanos
foo bar baz qux quux
lex lexer parse parser check checker interp compile compiler transpile transpiler runtime
prelude stdlib builtin builtins upvalue upvalues opcode opcodes bytecode chunk chunks
fiber fibers heap heaps handle handles slot slots
webserver middleware
oha hyperfine pgo lto
raylang ray
nth
""".split())

# Patrones de declaración por lenguaje (línea a línea; pragmático como el test).
RS_PATS = [
    ("fn",      re.compile(r"\bfn\s+([a-z][a-z0-9_]*)")),
    ("let",     re.compile(r"\blet\s+(?:mut\s+)?([a-z][a-z0-9_]*)")),
    ("struct",  re.compile(r"\bstruct\s+([A-Za-z][A-Za-z0-9_]*)")),
    ("enum",    re.compile(r"\benum\s+([A-Za-z][A-Za-z0-9_]*)")),
    ("trait",   re.compile(r"\btrait\s+([A-Za-z][A-Za-z0-9_]*)")),
    ("const",   re.compile(r"\bconst\s+([A-Z][A-Z0-9_]*)\s*:")),
    ("static",  re.compile(r"\bstatic\s+([A-Z][A-Z0-9_]*)\s*:")),
    ("param",   re.compile(r"[(,]\s*(?:mut\s+)?([a-z][a-z0-9_]*)\s*:")),
    ("campo",   re.compile(r"^\s+(?:pub\s+)?([a-z][a-z0-9_]*)\s*:\s")),
]
RAY_PATS = [
    ("fn",      re.compile(r"\bfn\s+([a-z][a-z0-9_]*)")),
    ("let",     re.compile(r"\blet\s+([a-z][a-z0-9_]*)")),
    ("var",     re.compile(r"\bvar\s+([a-z][a-z0-9_]*)")),
    ("struct",  re.compile(r"\bstruct\s+([A-Za-z][A-Za-z0-9_]*)")),
    ("enum",    re.compile(r"\benum\s+([A-Za-z][A-Za-z0-9_]*)")),
    ("trait",   re.compile(r"\btrait\s+([A-Za-z][A-Za-z0-9_]*)")),
    ("param",   re.compile(r"[(,]\s*([a-z][a-z0-9_]*)\s*:")),
    ("campo",   re.compile(r"^\s+([a-z][a-z0-9_]*)\s*:\s")),
]

def tokens_of(ident):
    # snake_case y CamelCase → tokens minúsculos
    parts = []
    for p in ident.split("_"):
        parts.extend(re.findall(r"[A-Z]?[a-z0-9]+", p))
    return [t.lower() for t in parts if t]

FALSOS_AMIGOS = {"error","errores","total","totales","final","temporal","temporales","color",
    "colores","animal","animales","division","modulo","persona","personas","auxiliar",
    "auxiliares","subtotal","normal","base"}
es_words -= FALSOS_AMIGOS

JERGA_EXTRA = set("""std app msg dir demo req resp cmd docs ufcs hdr hdrs tparams cargs cparams
targs metadata timestamp timestamps whitespace lookahead peephole subprocess stdout stderr stdin
lockfile manifest workspace toolchain codegen enqueue dequeue popped pushed parked woken spawned
joined deserialize serialize stringify bitwise xor shl shr endian bigint varint favicon localhost
keepalive backoff jitter typedefs mkdir rmdir chmod symlink realpath dirname basename utf8 sha256
sha512 base64 hexdump webhook websocket websockets bufreader vtable vtables mangled mangle
scrutinee arity arities desugar desugared lowering rewriter resolver namespacing stem leaf reexport
reexports frontmatter goldens fixture fixtures oneshot backpressure rendezvous scheduler
deterministic wasm playground superinstruction superinstructions monomorphic polymorphic
bidirectional exhaustiveness annotation annotations
""".split())
jargon.update(JERGA_EXTRA)

def english_like(tok):
    if tok in english or tok in jargon: return True
    # web2 no trae flexiones: probar plural/verbos comunes
    for suf in ("s", "es", "ed", "ing", "d"):
        if len(tok) > len(suf) + 2 and tok.endswith(suf) and tok[: -len(suf)] in english:
            return True
    if tok.endswith("ies") and tok[:-3] + "y" in english: return True
    if tok.endswith("ing") and tok[:-3] + "e" in english: return True
    return False

def classify(tok):
    if len(tok) < 3: return None
    if tok in es_words: return "es"          # confianza alta: wordlist
    if english and not english_like(tok):
        return "sospechoso"                   # ni inglés ni jerga → revisar
    return None

hits = []            # (archivo, linea, tipo_decl, ident, tokens_es, clase)
per_file = defaultdict(lambda: defaultdict(int))
per_token = defaultdict(int)

for d in DIRS:
    for dirpath, _, files in os.walk(os.path.join(ROOT, d)):
        if ".ray-deps" in dirpath or "fixtures" in dirpath: continue
        for name in sorted(files):
            ext = name.rsplit(".", 1)[-1]
            if ext not in ("rs", "ray"): continue
            path = os.path.join(dirpath, name)
            rel = os.path.relpath(path, ROOT)
            pats = RS_PATS if ext == "rs" else RAY_PATS
            try:
                lines = open(path, encoding="utf-8").read().splitlines()
            except Exception:
                continue
            in_string_block = False
            for i, line in enumerate(lines, 1):
                if "// es-ok" in line: continue
                code = line.split("//")[0]  # fuera comentarios de línea
                for kind, pat in pats:
                    for m in pat.finditer(code):
                        ident = m.group(1)
                        if ident in ("self",): continue
                        bad = {}
                        for t in tokens_of(ident):
                            c = classify(t)
                            if c: bad[t] = c
                        if bad:
                            clase = "es" if "es" in bad.values() else "sospechoso"
                            hits.append((rel, i, kind, ident, ",".join(sorted(bad)), clase))
                            per_file[rel][clase] += 1
                            for t in bad: per_token[t] += 1

# --- salida ---
out = sys.argv[1] if len(sys.argv) > 1 else "/tmp/arqueo_detalle.txt"
with open(out, "w") as f:
    for rel, i, kind, ident, toks, clase in hits:
        f.write(f"{rel}:{i}\t{kind}\t{ident}\t[{toks}]\t{clase}\n")

es_hits = [h for h in hits if h[5] == "es"]
sos_hits = [h for h in hits if h[5] == "sospechoso"]
print(f"TOTAL declaraciones señaladas: {len(hits)}  (es-wordlist: {len(es_hits)}, sospechosas-dict: {len(sos_hits)})")
print(f"archivos afectados: {len(per_file)}")
print("\n== top 25 archivos (es / sospechoso) ==")
ranked = sorted(per_file.items(), key=lambda kv: -(kv[1]['es'] + kv[1]['sospechoso']))
for rel, c in ranked[:25]:
    print(f"  {c['es']:4d} / {c['sospechoso']:4d}  {rel}")
print("\n== top 40 tokens ==")
for t, n in sorted(per_token.items(), key=lambda kv: -kv[1])[:40]:
    print(f"  {n:4d}  {t}")
print("\n== por tipo de declaración (solo 'es') ==")
kinds = defaultdict(int)
for h in es_hits: kinds[h[2]] += 1
for k, n in sorted(kinds.items(), key=lambda kv: -kv[1]):
    print(f"  {n:4d}  {k}")
print(f"\ndetalle completo: {out}")
