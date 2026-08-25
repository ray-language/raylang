# Publicar un paquete de raylang

La guía del **publicador** (M83a): de un directorio con código a un paquete que otros
instalan con `ray add`. Complementa al [`MANUAL.md`](MANUAL.md) §11 (que cubre el lado del
**consumidor**: imports, cápsulas, `ray.toml`, lock).

El modelo, en una frase: **no hay servidor** — un paquete es un repo git con un tag, y el
registro es otro repo git (el *índice*) que mapea `nombre → URL git + versiones + hash`.
Todo lo demás (verificación, reproducibilidad, retirada) se apoya en esas dos piezas.

## 1. Qué es un paquete

Un repo git con:

```
mipaquete/
├── ray.toml          # [package] name = "mipaquete", version = "1.0.0"
├── mod.ray           # la CARA del paquete (o la entrada declarada en `entry`)
└── …                 # los módulos internos que la cara reexporta
```

- **La cara**: `mod.ray` en la raíz — el paquete entero es una [cápsula](MANUAL.md#cápsulas-modray):
  el consumidor hace `import mipaquete;` (o `import mipaquete/submodulo;` si expones
  submódulos sueltos, como hacen `db`/`net`) y tus internos no reexportados quedan
  protegidos. Sin `mod.ray`, vale la entrada (`entry`, por defecto `src/main.ray`).
- **El nombre** (`[package] name`): letras, dígitos, `-` y `_`. Es el nombre con el que se
  importa y el archivo del índice (`<nombre>.toml`).
- **La versión**: semver `X.Y.Z` (con pre-releases `X.Y.Z-rc1` si hace falta, §4).
- Las **dependencias** del paquete se declaran como en cualquier proyecto; los consumidores
  las resuelven transitivamente. Ojo: se resuelven contra el índice DEL CONSUMIDOR (§6).

## 2. El índice

Un repo git (o un directorio local, para probar) con un TOML por paquete:

```toml
# <índice>/mipaquete.toml  — lo escribe `ray registry publish`; no se edita a mano
[1.0.0]
git = "git+https://github.com/user/mipaquete@v1.0.0"
hash = "sha256:…"
```

Se configura por proyecto en `ray.toml` — o por entorno, que tiene prioridad:

```toml
[registry]
index = "git+https://github.com/ray-language/ray-index@main"  # el índice OFICIAL (o un dir local)
```

```sh
export RAY_INDEX=/ruta/al/indice               # override (tests, CI)
```

Un índice **remoto** se clona/cachea en `.ray-deps/.index` (lo refresca `ray update`).

**Mirror de paquetes** (M90.1) — para CI tras un firewall o caída del hosting: un prefijo
de URL que reescribe la descarga de cada paquete a `prefijo/<url-sin-esquema>` (estilo
proxy de Go: el mirror sirve los mismos repos bajo su prefijo). NO es otro índice (mismo
índice, otra URL); el hash publicado verifica el contenido venga de donde venga, y si el
mirror falla se cae a la URL original con un aviso.

```toml
[registry]
mirror = "https://mirror.corp/git"             # o export RAY_MIRROR=… (prioridad)
```
Crear el tuyo es `git init` + push: no hay más ceremonia.

**La UI web del índice** (M84) también es estática y se genera desde los mismos TOML — en
raylang, con el generador del repo del lenguaje:

```sh
ray run tools/registry_site.ray <dir-del-índice> <dir-de-salida>   # index.html + p/<nombre>.html
```

En el CI del repo del índice (GitHub Pages), junto a la auditoría de firmas:

```yaml
# .github/workflows/site.yml del repo del ÍNDICE
on: { push: { branches: [main] } }
jobs:
  site:
    runs-on: ubuntu-latest
    permissions: { pages: write, id-token: write }
    steps:
      - uses: actions/checkout@v4
      - run: curl -sSfL https://raw.githubusercontent.com/roberto-ayala/raylang/main/install.sh | sh
      - run: ~/.local/bin/ray registry verify .          # la auditoría de firmas (M83)
      - run: ~/.local/bin/ray run tools/registry_site.ray . _site   # requiere el checkout del lenguaje o el script vendorizado
      - uses: actions/upload-pages-artifact@v3
      - uses: actions/deploy-pages@v4
```

## 3. `ray registry publish`, paso a paso

```sh
git tag v1.0.0 && git push --tags     # la versión del ray.toml, con prefijo v
ray registry publish                            # …o: ray registry publish --repo git+URL@ref
# Para consumo ANÓNIMO publica con URL https (--repo "git+https://github.com/…@vX.Y.Z"):
# sin --repo, la URL sale del remoto `origin` — si empujas por ssh, el índice queda con una URL
# ssh que solo clona quien tiene clave (M134). Los paquetes del ecosistema viven en la
# organización github.com/ray-language; el índice oficial es ray-language/ray-index.
```

Sin `--repo`, la spec publicada se deriva del remoto **`origin`** + el tag **`v<versión>`**
(que debe existir: se publica un commit fijado, no una rama). Antes de tocar el índice,
`publish` valida **el contenido real de esa ref** (un clon limpio temporal — no tu working
tree, así que *commitea antes de taggear*):

1. nombre y versión bien formados;
2. la **cara** existe en el clon (`mod.ray` o la entrada);
3. **todos** los `.ray` publicados lexean y parsean (también los no importados);
4. el paquete **supera el check semántico completo** (con sus deps resueltas y la `std/`;
   sin exigir `main`: un paquete es una librería);
5. se calcula el **hash SHA-256 del contenido** — lo que el lock de cada consumidor
   re-verificará en cada instalación.

Si todo pasa, añade la entrada al índice y la commitea. **Las versiones son inmutables**:
re-publicar `1.0.0` es un error — publica `1.0.1` (o retira con `yank`, §5).

## 4. Versionado: qué instala cada requisito

| El consumidor declara | Instala |
|---|---|
| `"1.2.0"` | exactamente 1.2.0 |
| `"^1.2"` | la mayor `1.x` ≥ 1.2 (compatible semver) |
| `"~1.2.3"` | la mayor `1.2.x` ≥ 1.2.3 |
| `"*"` (o `ray add` sin versión) | la mayor **final** publicada |

- Con dependencias transitivas gana **la mayor compatible** (MVS ligero), y el lock la
  **fija** (URL + hash): builds reproducibles hasta que un `ray update` re-resuelva.
- **Pre-releases** (regla de cargo): `^1.0` **jamás** elige `1.1.0-rc1`; una pre solo se
  instala si el requisito la menciona con el triple completo (`1.3.0-rc1` o `^1.3.0-rc1`).
- Contrato semver de siempre: parche = fixes, minor = API aditiva, major = rupturas. La
  cara (`mod.ray`) ES tu API pública — lo no reexportado puede cambiar libremente.

## 5. Retirar una versión: `ray registry yank`

```sh
ray registry yank mipaquete@1.0.1          # la resolución deja de elegirla (^1.0 la salta)
ray registry yank mipaquete@1.0.1 --undo   # la restaura
```

`yank` **no borra** (quien ya la tiene fijada en su lock sigue compilando; el requisito
EXACTO también la encuentra): retira la versión de la resolución por rangos. Es la
herramienta para "esta versión tiene un bug/vulnerabilidad — no se la des a nadie nuevo".

## 6. Las garantías (y sus límites)

- **Integridad**: el lock fija URL + SHA-256 por dependencia y el propio índice se
  verifica por hash (TOFU) — contenido alterado = error de supply-chain, en seco.
- **Reproducibilidad**: mismas entradas del lock → mismos bytes, en cualquier máquina.
- **Índice único por proyecto**: las transitivas por nombre se resuelven contra TU
  índice; si una dependencia declara un índice propio distinto, el CLI **avisa**
  (mitigación de *dependency confusion*).
- **Dueños y firmas** (M83b/c): ver §6bis — con `--sign`, el nombre queda reclamado y
  cada versión va firmada; un índice manipulado deja de resolver.

## 6bis. Dueños de nombre y firmas (`--sign`)

Para abrir un índice a varios publicadores:

```sh
ray registry keygen                        # una vez: genera tu clave Ed25519 (~/.ray/publish.key o RAY_KEY)
ray registry publish --sign                # publica firmado
```

- **La primera publicación firmada RECLAMA el nombre**: se escribe el sidecar
  `<nombre>.owners.toml` en el índice (tu handle de `git config user.name` + tu clave
  pública, fijada por TOFU). Desde entonces, `--sign` con otra clave sobre ese nombre es
  **error** — commitea el sidecar junto a la entrada.
- **La firma** es Ed25519 sobre `nombre@versión:hash` y va en la entrada del índice
  (`sig = "ed25519:…"`). El consumidor la **verifica al resolver**: una firma que no casa
  con el dueño registrado corta la resolución en seco (protege incluso si el repo del
  índice se compromete — el atacante puede cambiar URL y hash, pero no firmar por ti).
  Una entrada sin firma de un paquete con dueño resuelve con **aviso** (transición).
- **`ray registry verify <dir>`** audita el índice completo (cada firma verifica contra su
  dueño): es el check de **CI del repo del índice**. La otra mitad del enforcement — que
  un PR solo toque paquetes de su autor — es del hosting (CODEOWNERS/branch protection +
  publicación por PR).
- Guarda la clave: es tu identidad de publicador. No hay revocación v1 (perderla =
  reclamar un nombre nuevo o editar el sidecar por PR con consenso del índice).

## 7. Receta mínima de punta a punta

```sh
# El paquete
ray new textutils && cd textutils
echo 'pub fn shout(s: string) -> string { s.to_upper() + "!" }' > mod.ray
git init -q && git add -A && git commit -qm "v0.1.0" && git tag v0.1.0
git remote add origin git@github.com:user/textutils.git && git push -q --tags origin main

# (opcional pero recomendado) tu clave de publicador
ray registry keygen

# El índice (una vez)
git init -q ~/ray-index && (cd ~/ray-index && git commit -qm raiz --allow-empty)

# Publicar
export RAY_INDEX=~/ray-index
ray registry publish --sign

# Consumir (cualquier proyecto con el mismo índice)
ray add textutils          # ray.toml: textutils = "0.1.0"; descarga y fija en el lock
```

```rust
import textutils;
fn main() { print(textutils.shout("hola")); }   // HOLA!
```
