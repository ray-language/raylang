# Notes (mobile)

Una app de notas para iOS y Android escrita en raylang, con la interfaz en React + TypeScript y
los datos en `std/kv`. Es el proyecto del capítulo
[App móvil para iOS y Android](../../../handbook/movil.md) del handbook, que lo explica paso a paso.

```sh
npm --prefix frontend install
ray test                         # el modelo y el protocolo, sin ventana
ray dev                          # en el escritorio, con recarga en caliente de la interfaz
ray bundle --ios                 # proyecto Xcode (macOS)
ray bundle --android             # proyecto Gradle
```
