# packages/zip — archivos ZIP sobre el disco (M419)

Lectura y escritura de archivos ZIP **sobre un handle de archivo**, en raylang puro (cero deps):
el directorio central se lee de la cola con `fs.seek`, cada entrada se lee y descomprime por
partes (con el `std/inflate` incremental del runtime), **ZIP64** (más de 65 535 entradas, tamaños
y posiciones de más de 4 GiB), nombres UTF-8 y CP437, modo Unix, hora local del sistema
(`std/time.local_offset_ms`, también en Windows). Nace del código de rayzip, que ya leían `unzip`,
`tar.exe` y `Expand-Archive`. `std/zip` sigue siendo el lector en memoria para lo pequeño
(`.sublime-package`, `.jar`).

```rust
import zip/zip;

// Escribir: deflate o stored (lo que ocupe menos), hora local, modo Unix, UTF-8.
let w = zip.create("out.zip")?;
zip.add_dir(w, "docs")?;
zip.add_bytes(w, "docs/readme.txt", "hola\n".to_bytes())?;
zip.add_file(w, "docs/photo.jpg", "/tmp/photo.jpg")?;   // > 512 MiB → stored por trozos
let s = zip.finish(w)?;                                  // Summary { entries, bytes_in, bytes_out }

// Leer: sin cargar el archivo; cada entrada verificada (tamaño y CRC-32).
let a = zip.open("out.zip")?;
for e in zip.entries(a) {
    print("${e.name} ${e.size} ${zip.mtime_ms(e)}");
}
let data = zip.read(a, zip.find(a, "docs/readme.txt").unwrap())?;
zip.read_with(a, big, fn(chunk: bytes) -> Result<int, string> { … })?;  // por trozos
zip.close(a);
```

- **Opciones** (`zip.create_with(path, zip.Options { level, deflate_limit, clock })`;
  `zip.options()` da los defaults): nivel 1–9 (0 = todo stored), tamaño a partir del cual
  `add_file` almacena por trozos (512 MiB), y el reloj de las fechas DOS — `zip.dostime_local()`
  (la hora local del sistema, como `zip` y el Explorador) o `zip.dostime_utc()` (archivos
  reproducibles, los mismos octetos en cualquier máquina).
- **Entradas** (`zip.Entry`, de `zip/format`): `name`, `size`, `compressed`, `method` (0 stored, 8
  deflate), `crc`, `offset`, `mode`, `is_dir`; `format.is_symlink(e)`, `format.permissions(e)`;
  `zip.mtime_ms(e)` (el campo extendido UT en UTC si lo hay; si no, la fecha DOS en hora local).
- **Lo que no hace**: cifrado y otros métodos de compresión (`Err` claro al leer), ni extraer a
  disco (nombres hostiles, symlinks, permisos: eso es de la app — rayzip lo hace).
- **Escritura segura**: el archivo se escribe en `<path>.zip-tmp` y se renombra en `finish`; un
  fallo nunca deja medio archivo. `zip.abort(w)` lo descarta.

## Licencia

[Apache License 2.0](LICENSE). Copyright 2026 Roberto Ayala.
