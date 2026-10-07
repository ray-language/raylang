# `oidc` — el lado cliente de OpenID Connect (paquete adicional, **no** embebido)

Para que una app raylang **inicie sesión con un proveedor OIDC** (rayauth, Keycloak, Auth0,
Google, Entra ID…) o **acepte sus tokens** como servidor de recursos. Escrito en raylang sobre
`net/http`, `std/crypto`, `std/json` y `std/base64`; sin runtime nuevo. Tier 2 del ecosistema
(DESIGN §53). Origen: el paquete que rayauth escribió a mano para sus apps de ejemplo (hallazgo
R37), probado de punta a punta en un navegador real.

## Cómo usarlo

```sh
ray add oidc
```

```toml
[dependencies]
oidc = "^0.1.0"
```

## El flujo de inicio de sesión (relying party)

```rust
import std/url;
import oidc/login;
import oidc/provider;

// Una vez, al arrancar: descubrimiento + JWKS.
let p = provider.discover("https://id.example.com").unwrap();
let c = login.Client {
    client_id: "mi-app",
    client_secret: "…",          // "" para un cliente público (SPA, app nativa): PKCE basta
    redirect_uri: "https://mi-app.example.com/callback",
    scope: "openid profile email",
};

// GET /login: guarda `pending` en la sesión y redirige al navegador a `pending.url`.
let pending = login.begin(p, c);

// GET /callback?code=…&state=…&iss=…: coteja `state`, canjea el código (PKCE S256) y verifica el ID token.
let t = login.finish(p, c, pending, url.parse_query(req.query))?;
print(t.claims.sub + " " + t.claims.email);   // y t.access_token, t.refresh_token, t.id_token

let r = login.refresh(p, c, t.refresh_token)?;          // tokens nuevos (ID token verificado sin nonce)
let info = login.userinfo(p, t.access_token)?;          // claims del userinfo endpoint (JSON)
let bye = login.logout_url(p, t.id_token, "https://mi-app.example.com/")?;   // logout iniciado por el RP
```

`begin_with(p, c, extra)` añade parámetros de autorización (`prompt`, `login_hint`,
`ui_locales`…). `finish_at`/`refresh_at` toman el reloj como parámetro (tests).

## Servidor de recursos: aceptar un access token

```rust
import oidc/tokens;

// En cada petición, sobre el bearer token (RFC 9068: `typ: at+jwt`; un ID token nunca pasa).
let claims = tokens.verify_access_token(p, bearer, ["read"], tokens.now())?;
// claims.sub, claims.client_id, claims.scope, claims.raw (el payload entero)
```

`tokens.verify_id_token(p, client_id, token, nonce, now_ms)` verifica un ID token (firma, `iss`,
`exp`/`nbf`, `aud`, `azp` con varias audiencias, `nonce`); `tokens.verify_jwt(p, token, now_ms)` es
el bloque común para otros JWT que firme el proveedor (logout tokens, aserciones).

## Qué verifica y qué no

- **Firma contra el JWKS** del proveedor: RS256 (RSA), ES256 (P-256) y EdDSA (Ed25519). El
  `alg` de la cabecera debe ser el que implica el tipo de clave: ni `none` ni confusión de
  algoritmos. Un `kid` desconocido refresca el JWKS **una vez** (rotación de claves).
- **Descubrimiento**: el documento debe nombrar exactamente el issuer pedido.
- **Callback**: `state` contra lo guardado, `iss` cuando el proveedor lo envía (RFC 9207), y el
  `error`/`error_description` del proveedor como `Err` legible.
- No cubre: `response_mode=form_post`, el flujo implícito (desaconsejado), DPoP, client
  assertions `private_key_jwt`, ni la validación de `at_hash`/`c_hash`.

## Módulos

| módulo | qué |
|---|---|
| `oidc/provider` | `Provider` (issuer, endpoints, JWKS) · `discover(issuer)` · `with_keys(…)` (a mano, sin red) · `refresh_keys(p)` · `key_of(p, kid)` · `get_text(url)` · `TIMEOUT_MS` |
| `oidc/login` | `Client`, `Pending`, `Tokens` · `begin`/`begin_with` · `finish`/`finish_at` · `refresh`/`refresh_at` · `userinfo` · `logout_url` · `challenge_of` (PKCE S256) |
| `oidc/tokens` | `Claims` · `verify_id_token` · `verify_access_token` · `verify_jwt` · `claims_of` · `audience_has` · `now` · `LEEWAY_S` |
| `oidc/jose` | JWS compacto contra una JWK: `verify(token, jwk)` · `header_of` · `payload_unverified` · `key_in(jwks, kid)` |

La prueba de punta a punta (`tests/oidc_cli.rs`) levanta un proveedor de juguete en raylang y
recorre el flujo entero, con sus caminos de error, en la VM y en el binario nativo.

## Licencia

Apache-2.0 (ver `LICENSE`).
