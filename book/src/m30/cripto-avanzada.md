# Criptografía avanzada: cifrado autenticado y firmas

Hasta aquí el dominio cripto era **hashing** y **HMAC** (M20): funciones de un solo sentido y
autenticación con clave compartida. Faltaban las dos piezas que hacen segura una conexión de
verdad: **cifrar** un mensaje para que solo el destinatario lo lea, y **firmar**lo para que
cualquiera pueda comprobar quién lo escribió sin conocer su secreto. M30 cierra el dominio con
las dos primitivas modernas de referencia —ChaCha20-Poly1305 y Ed25519—, ambas escritas enteras
en raylang y verificadas contra los vectores de su RFC.

Pero el hilo conductor de M30 no es criptográfico: es **el pago de una deuda de lenguaje**. En
M28.3 raylang ganó enteros con tamaño (`u32`, `u64`), y M30 es su gran demostración.

## M30.1 — ChaCha20-Poly1305: el showcase de los enteros con tamaño

ChaCha20 (RFC 8439) es el cifrador de flujo de Bernstein: veinte rondas de suma, XOR y rotación
sobre palabras de 32 bits. Genera un flujo de clave pseudoaleatorio que se XORea con el texto —
cifrar y descifrar son la misma operación—. Escribirlo destapa por qué M28.3 importaba tanto.
Compara. En `sha256.ray`, cada suma que puede desbordar arrastra su máscara:

```raylang
h0 = (h0 + a) & 4294967295   // & 0xFFFFFFFF, una y otra vez
```

En `chacha20.ray`, con `u32`, el wrapping es parte del tipo y desaparece del código:

```raylang
fn rotl32(x: u32, n: u32) -> u32 { (x << n) | (x >> (32 - n)) }

fn quarter_round(st: [u32], a: int, b: int, c: int, d: int) {
    st[a] = st[a] + st[b];   st[d] = rotl32(st[d] ^ st[a], 16);
    st[c] = st[c] + st[d];   st[b] = rotl32(st[b] ^ st[c], 12);
    // ...
}
```

No hay ni un solo `& 0xFFFFFFFF`. El código es **idéntico al pseudocódigo del RFC**, línea por
línea. Esa es la lección: un tipo bien elegido no añade potencia —siempre se pudo enmascarar a
mano— pero borra una clase entera de ruido y de errores por omisión.

Poly1305, el MAC que acompaña a ChaCha20, sube la apuesta. Es aritmética modular de 130 bits
(módulo 2¹³⁰−5), y el port de referencia (`poly1305-donna`) la representa en **cinco limbs de 26
bits** cuyos productos crecen hasta 52-55 bits. Eso pide `u64` nativo; antes habría que emular
64 bits a mano sobre el `int` de 63. La selección final del resultado se hace en **tiempo
constante** (con la máscara `(g4 >> 63) - 1`, sin un `if` que filtre por el tiempo de ejecución
qué rama se toma) —un detalle que no es adorno: en cripto, una rama observable es una fuga—.

Poly1305 destapó además un hueco del propio lenguaje. La coerción de un literal `uint` a un tipo
con tamaño solo funcionaba en la inicialización; al escribir `x = 200` con `x: u8` fallaba. Se
extendió `check_expr_expected` a la **asignación** (variable, campo, elemento). Otra vez el
patrón: escribir cripto real ejercita el lenguaje y aflora sus asperezas.

M30.1c compone las dos en el **AEAD** (*Authenticated Encryption with Associated Data*): `seal`
cifra con ChaCha20 (contador 1) y autentica con Poly1305, usando como clave de una sola vez el
bloque ChaCha20 con contador 0; el tag cubre no solo el criptograma sino el AAD y las longitudes.
`open` verifica el tag **antes** de descifrar y devuelve `None` si algo fue manipulado. Cifrado
que además detecta manipulación: eso es lo que quieres en la práctica.

## M30.2 — Ed25519: la parte más matemática del proyecto

Firmar es asimétrico: el firmante tiene una clave privada, el mundo verifica con la pública, y de
la pública no se puede derivar la privada. Ed25519 (RFC 8032) lo consigue con aritmética sobre una
**curva de Edwards** en el campo módulo 2²⁵⁵−19. Es, con diferencia, lo más matemático que se ha
escrito en raylang, y su prerrequisito es **SHA-512** (M30.2a) —otro showcase de `u64`, como
SHA-256 pero sin máscaras, con las constantes de 64 bits compuestas vía `w64(hi, lo)` porque
superan `i64::MAX`—.

`ed25519.ray` es un **port de TweetNaCl**, la implementación de referencia más compacta y auditada
que existe (dominio público, apenas cien líneas de C denso). La decisión de diseño clave es la
representación del campo: **16 limbs de 16 bits sobre un `[int]` con signo** (i64). ¿Por qué con
signo, y por qué caben? Porque los productos de la multiplicación escolar y el pliegue del
reducido (2²⁵⁶ ≡ 38) se quedan holgados dentro de i64 —no hace falta emular u128—, y los *carries*
se propagan con **desplazamiento aritmético** de i64, que respeta el signo. Sobre ese campo se
montan la ley de grupo de Edwards en coordenadas extendidas (`point_add`, `scalarmult`), el
empaquetado de puntos con raíz cuadrada modular (`pow2523`) y la firma/verificación con reducción
de escalares módulo L.

### El acertijo del vector "mal transcrito"

Durante la depuración apareció un fallo tozudo: la firma de raylang no coincidía con el valor
"esperado" de un vector de test. La reacción instintiva es buscar el bug en tu código. Pero al
cruzar **tres implementaciones independientes** —raylang, una referencia propia y el apéndice del
propio RFC 8032— las tres coincidían byte a byte entre sí y **discrepaban del esperado**. El bug
no estaba en el código: estaba en el vector, mal transcrito a mano.

La lección es de método, no de criptografía: cuando tu resultado choca con un valor de referencia,
verifica contra la **fuente autoritativa**, no contra una copia memorizada o retecleada. Y un
corolario bonito: que tres implementaciones nacidas por separado converjan es la mejor evidencia
de que las tres son correctas.

## M30.3 — JWT EdDSA: firma asimétrica de tokens

El HS256 de M20 firmaba tokens JWT con HMAC: **simétrico**, el que firma y el que verifica comparten
el mismo secreto. M30.3 añade firma **asimétrica** reusando Ed25519 (RFC 8037, `alg: "EdDSA"`).
Entre las opciones estándar —RS256 (RSA), ES256 (curva P-256)— se eligió EdDSA por una razón
puramente de reutilización: encaja directo sobre M30.2, mientras que RSA (exponenciación modular
gigantesca) o P-256 (otra curva, otro ECDSA) serían cada uno un módulo entero aparte.

El token es `base64url(header).base64url(payload).base64url(firma)`, con la firma = Ed25519 sobre
`"header.payload"`. `jwt_eddsa_sign(seed, claims)` la produce con la semilla privada;
`jwt_eddsa_verify(pubkey, token)` la comprueba con la clave pública —cualquiera puede verificar,
solo el poseedor de la semilla puede firmar—. Se validó de la forma más exigente posible: el token
resulta **byte-idéntico** al que produce un cómputo independiente en Python. No solo "verifica bajo
mi propia implementación", sino **interoperable** con el ecosistema.

---

M30 cierra el dominio cripto: cifrado autenticado (ChaCha20-Poly1305), firma (Ed25519) y JWT
asimétrico (EdDSA), todo como librería raylang pura, verificado contra los vectores de cada RFC en
los dos motores. Quedan fuera AES-GCM (exige S-boxes y GHASH sobre GF(2¹²⁸)) y RS256/ES256 (RSA y
P-256, cada uno un módulo propio). Pero la moraleja del capítulo no es la lista de primitivas: es
que la criptografía moderna, cuando el lenguaje tiene los **enteros del tamaño justo**, se escribe
tal como está en el papel.
