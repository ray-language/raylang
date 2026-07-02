# La biblioteca estándar de raylang (`std/`)

Módulos de biblioteca **escritos en raylang**, importables con la sintaxis de módulos por ruta:

```raylang
import std/math;

fn main() -> int {
    print(math.gcd(48, 36)); // 12
    0
}
```

A diferencia del **prelude** (que se inyecta automáticamente en cada programa: `Option`/`Result`,
`map`/`filter`/`fold`, `Set`/`Deque`/`StringBuilder`, `Iterator`, …), la `std/` es **opcional**: solo
se carga lo que se importa.

## Descubrimiento

El binario localiza `std/` en tiempo de ejecución:

1. La variable de entorno `RAYLANG_STD` (apuntando al directorio que contiene `std/`, o a `std/`).
2. Si no, subiendo desde el ejecutable (en el repo: `target/…/ray` → la raíz del proyecto con `std/`).

Los módulos son **públicos** (`pub`) y están documentados con comentarios `///`; genera su documentación
con `ray doc std/math.ray`.

## Módulos

- **`std/math`** — utilidades enteras que complementan los builtins matemáticos: `iabs`, `sign`,
  `clamp`, `gcd`, `lcm`, `ipow`, `factorial`, `is_prime`.
- **`std/text`** — utilidades de string más allá de los builtins: `is_empty`, `pad_left`, `pad_right`,
  `capitalize`, `reverse`, `count`, `words`.

(Más módulos por venir; ver DESIGN §42.7.)
