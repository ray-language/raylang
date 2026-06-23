# Métodos por defecto

Un trait suele tener métodos que casi todos los tipos implementarían igual. Obligar a cada
impl a repetirlos es ruido. Un **método por defecto** da el comportamiento "de fábrica": la
firma trae cuerpo, y un impl que no lo redefina lo hereda.

```rust
trait Saludo {
    fn nombre(self) -> string;            // requerido: cada tipo lo da

    fn saludar(self) -> string {          // por defecto: puede usar otros métodos
        self.nombre()
    }
}

impl Saludo for Persona {
    fn nombre(self) -> string { self.n }
    // 'saludar' no se implementa → se hereda el cuerpo por defecto
}
```

Un impl puede **redefinir** un método por defecto (su versión gana), y un método por defecto
puede llamar a otros métodos del trait sobre `self` —como `saludar` llama a `nombre`—.

## La implementación: una síntesis, nada más

M9.3a no añade ningún mecanismo nuevo: reusa la bajada de M9.1. Recordemos que allí cada
método de un `impl` se **baja** a una función ordinaria con nombre manglado `Tipo#metodo`.
Un método por defecto es exactamente eso, pero su cuerpo viene del **trait** en vez del impl:

```text
trait Saludo { fn saludar(self) -> string { self.nombre() } }
impl Saludo for Persona { fn nombre(self) -> string { self.n } }   // no da 'saludar'
        │
        ▼  (el checker sintetiza el método que falta, desde el defecto)
fn «Persona#saludar»(self: Persona) -> string { self.nombre() }
```

El checker, al bajar cada impl, mira qué métodos del trait **no** están en el impl pero
**tienen** cuerpo por defecto, y los sintetiza: una función manglada `Tipo#metodo` con el
cuerpo del defecto y `Self` = el tipo destino. De ahí en adelante es un método como
cualquier otro: entra en la tabla de resolución, se verifica su cuerpo (con `Self` en
ámbito), y `self.nombre()` dentro de él se resuelve por el tipo concreto.

Dos ajustes acompañan a la síntesis:

- La **cobertura** se relaja: un impl al que le falte un método solo es error si ese método
  **no** tiene defecto. Con defecto, simplemente se hereda.
- La **redefinición** es automática: si el impl da el método, se baja el del impl y el
  defecto no se sintetiza. El del impl gana sin reglas especiales.

## Compone con todo

Como el método por defecto está en la lista de métodos del trait, también funciona a través
de un **bound** (M9.2): un genérico `fn anunciar<T: Saludo>(x: T)` puede llamar `x.saludar()`,
y el diccionario que se pasa en el sitio de llamada incluye la versión sintetizada para el
tipo concreto.

Y, como todo lo de M9 hasta aquí, es **erasure**: el método sintetizado es una función
ordinaria; el intérprete y la VM no saben que hubo un defecto. **Runtime intacto**, una vez
más.

> Con M9.1 (impls concretos), M9.2 (bounds) y M9.3a (defectos), todo el despacho de raylang
> sigue siendo **estático**: se resuelve en tiempo de chequeo. La última pieza —resolver un
> método sobre un valor cuyo tipo concreto **no** se conoce hasta runtime— es el **trait
> object** (M9.3b), y es la única que obliga a tocar los motores.
