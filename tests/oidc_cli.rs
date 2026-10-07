//! R37 (rayauth): el paquete `oidc` — el lado CLIENTE de OpenID Connect — contra un proveedor de
//! juguete escrito en raylang en el mismo programa: discovery, código + PKCE S256 + nonce + `iss`
//! (RFC 9207), canje con secreto (HTTP Basic), ID token y access token (RFC 9068) verificados
//! contra el JWKS (EdDSA), refresh, userinfo, logout, y los caminos de error (state, denegación,
//! scope, tipo, caducidad, audiencia, firma manipulada, secreto erróneo). VM y nativo.

use std::path::PathBuf;
use std::process::Command;

fn has_rustc() -> bool {
    Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

#[test]
fn the_oidc_client_signs_in_against_a_toy_provider() {
    let d = std::env::temp_dir().join(format!("raylang_test_oidc_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let root = env!("CARGO_MANIFEST_DIR");
    std::fs::write(
        d.join("ray.toml"),
        format!("[package]\nname = \"oidc_e2e\"\nversion = \"0.1.0\"\nentry = \"prog.ray\"\n\n[dependencies]\nnet = \"path:{root}/packages/net\"\noidc = \"path:{root}/packages/oidc\"\n"),
    )
    .unwrap();
    std::fs::write(d.join("prog.ray"), r#"// Proveedor OIDC de juguete + cliente `oidc` contra él, en el mismo programa.
import std/base64;
import std/crypto;
import std/json;
import std/net;
import std/url;
import net/webserver;
import oidc/jose;
import oidc/login;
import oidc/provider;
import oidc/tokens;

fn b64(b: bytes) -> string { base64.base64url(b) }

fn err_of<T>(r: Result<T, string>) -> string {
    match (r) {
        Result.Ok(_) => "(ok?!)",
        Result.Err(e) => e,
    }
}

// JWT EdDSA con kid "k1".
fn sign(seed: bytes, typ: string, payload: string) -> string {
    let head = b64(json.obj()
        .field("alg", "EdDSA")
        .field("kid", "k1")
        .field("typ", typ)
        .render()
        .to_bytes());
    let input = head + "." + b64(payload.to_bytes());
    input + "." + b64(crypto.ed25519_sign(seed, input.to_bytes()).unwrap())
}

fn json_response(status: int, body: string) -> webserver.Response {
    var r = webserver.text(status, body);
    r.headers.insert("Content-Type", "application/json");
    r
}

fn main() -> int {
    let seed = crypto.random_bytes(32);
    let pubkey = crypto.ed25519_public_key(seed).unwrap();
    let jwks = json.obj()
        .field("keys", json.list(
        [
            json.obj()
                .field("kty", "OKP")
                .field("crv", "Ed25519")
                .field("kid", "k1")
                .field("x", b64(pubkey))
        ]
    ))
        .render();
    let srv = net.tcp_listen("127.0.0.1", 0).unwrap();
    let issuer = "http://127.0.0.1:" + to_string(net.local_port(srv));
    let stop: Channel<int> = Channel.new();
    let now_s = 1900000000;
    // El proveedor: discovery, jwks, authorize (devuelve el salto del navegador como JSON), token,
    // userinfo, end_session. Recuerda el code_challenge del último authorize.
    let issued: Channel<string> = Channel.bounded(8);
    spawn(fn() {
        let opts = webserver.options().with_stop(stop).with_drain(500).quiet();
        let _ = webserver.serve_options_on(srv, opts, fn() -> fn(webserver.Request) -> webserver.Response {
            fn(req: webserver.Request) -> webserver.Response {
                let q = url.parse_query(req.query);
                if (req.path == "/.well-known/openid-configuration") {
                    json_response(
                        200,
                        json.obj()
                            .field("issuer", issuer)
                            .field("authorization_endpoint", issuer + "/authorize")
                            .field("token_endpoint", issuer + "/token")
                            .field("userinfo_endpoint", issuer + "/userinfo")
                            .field("end_session_endpoint", issuer + "/logout")
                            .field("jwks_uri", issuer + "/jwks")
                            .render()
                    )
                } else if (req.path == "/jwks") {
                    json_response(200, jwks)
                } else if (req.path == "/authorize") {
                    // El código lleva reto y nonce (las fibras no comparten heap: un `var` no serviría).
                    let code = q.get_or("code_challenge", "") + "|" + q.get_or("nonce", "");
                    let ok = q.get_or("response_type", "") == "code"
                        && q.get_or("code_challenge_method", "") == "S256"
                        && q.get_or("client_id", "") == "app"
                        && q.get_or("prompt", "") == "login";
                    json_response(
                        200,
                        json.obj()
                            .field("code", code)
                            .field("state", q.get_or("state", ""))
                            .field("iss", issuer)
                            .field("ok", ok)
                            .render()
                    )
                } else if (req.path == "/token") {
                    let f = url.parse_query(from_utf8(req.body).unwrap_or(""));
                    let auth = req.headers.get_or("authorization", "");
                    let creds = from_utf8(base64.base64_decode(auth.strip_prefix("Basic ").unwrap_or("")).unwrap_or(b"")).unwrap_or(
                        ""
                    );
                    if (creds != "app:s3cret") {
                        return json_response(
                            401,
                            json.obj().field("error", "invalid_client").render()
                        );
                    }
                    let grant = f.get_or("grant_type", "");
                    let code_parts = f.get_or("code", "").split("|");
                    let challenge = code_parts[0];
                    let nonce = if (code_parts.len() > 1) { code_parts[1] } else { "" };
                    if (grant == "authorization_code") {
                        if (challenge == ""
                            || login.challenge_of(f.get_or("code_verifier", "")) != challenge) {
                            return json_response(
                                400,
                                json.obj()
                                    .field("error", "invalid_grant")
                                    .field("error_description", "bad code or verifier")
                                    .render()
                            );
                        }
                    } else if (grant != "refresh_token" || f.get_or("refresh_token", "") != "R3FRESH") {
                        return json_response(
                            400,
                            json.obj().field("error", "unsupported_grant_type").render()
                        );
                    }
                    let id_claims = json.obj()
                        .field("iss", issuer)
                        .field("sub", "ada")
                        .field("aud", "app")
                        .field("name", "Ada")
                        .field("email", "ada@example.test")
                        .field("exp", now_s + 300)
                        .field("iat", now_s);
                    let id_token = sign(
                        seed,
                        "JWT",
                        if (grant == "authorization_code") {
                            id_claims.field("nonce", nonce)
                        } else {
                            id_claims
                        }.render()
                    );
                    let at = sign(
                        seed,
                        "at+jwt",
                        json.obj()
                            .field("iss", issuer)
                            .field("sub", "ada")
                            .field("aud", "api")
                            .field("client_id", "app")
                            .field("scope", "openid profile read")
                            .field("exp", now_s + 300)
                            .render()
                    );
                    send(issued, at);
                    json_response(
                        200,
                        json.obj()
                            .field("id_token", id_token)
                            .field("access_token", at)
                            .field("token_type", "Bearer")
                            .field("expires_in", 300)
                            .field("refresh_token", if (grant == "authorization_code") { "R3FRESH" } else { "" })
                            .render()
                    )
                } else if (req.path == "/userinfo") {
                    if (!req.headers.get_or("authorization", "").starts_with("Bearer ")) {
                        return json_response(401, "{}");
                    }
                    json_response(
                        200,
                        json.obj().field("sub", "ada").field("preferred_username", "ada").render()
                    )
                } else {
                    webserver.text(404, "nope")
                }
            }
        });
    });
    // --- el cliente ---
    let p = provider.discover(issuer).unwrap();
    print("discover: " + to_string(p.token_endpoint == issuer + "/token"));
    let c = login.Client {
        client_id: "app",
        client_secret: "s3cret",
        redirect_uri: "http://127.0.0.1:1/cb",
        scope: "openid profile email"
    };
    var extra: Map<string, string> = Map.new();
    extra.insert("prompt", "login");
    let pending = login.begin_with(p, c, extra);
    // El «navegador»: GET al authorize (el proveedor de juguete devuelve el salto como JSON).
    let hop = json.parse(provider.get_text(pending.url).unwrap()).unwrap();
    print("authorize accepted: " + to_string(json.get_bool(hop, "ok").unwrap_or(false)));
    var cb: Map<string, string> = Map.new();
    cb.insert("code", json.get_string(hop, "code").unwrap_or(""));
    cb.insert("state", json.get_string(hop, "state").unwrap_or(""));
    cb.insert("iss", json.get_string(hop, "iss").unwrap_or(""));
    let t = match (login.finish_at(p, c, pending, cb, now_s * 1000)) {
        Result.Ok(x) => x,
        Result.Err(e) => {
            print("finish err: " + e);
            return 1;
        },
    };
    print(
        "signed in: "
            + t.claims.sub
            + " "
            + t.claims.email
            + " refresh="
            + t.refresh_token
            + " expires_in="
            + to_string(t.expires_in)
    );
    // Un callback con otro state, y uno con error del proveedor.
    var bad: Map<string, string> = Map.new();
    bad.insert("code", "x|y");
    bad.insert("state", "otro");
    print("state mismatch: " + err_of(login.finish_at(p, c, pending, bad, now_s * 1000)));
    var denied: Map<string, string> = Map.new();
    denied.insert("error", "access_denied");
    denied.insert("error_description", "user said no");
    print("denied: " + err_of(login.finish_at(p, c, pending, denied, now_s * 1000)));
    // El access token (RFC 9068) con sus scopes; un ID token no pasa por access token.
    let at = tokens.verify_access_token(p, t.access_token, ["read"], now_s * 1000).unwrap();
    print(
        "access token: "
            + at.sub
            + " client="
            + at.client_id
            + " scopes="
            + to_string(at.scope.len())
    );
    print(
        "missing scope: "
            + err_of(tokens.verify_access_token(p, t.access_token, ["admin"], now_s * 1000))
    );
    print(
        "id token as access token: "
            + err_of(tokens.verify_access_token(p, t.id_token, [], now_s * 1000))
    );
    print(
        "expired: "
            + err_of(tokens.verify_access_token(p, t.access_token, [], (now_s + 400) * 1000))
    );
    // Un ID token para otro cliente, y uno manipulado.
    print(
        "other audience: "
            + err_of(tokens.verify_id_token(p, "otra-app", t.id_token, pending.nonce, now_s * 1000))
    );
    let parts = t.id_token.split(".");
    let forged_payload = "{\"iss\":\""
        + issuer
        + "\",\"sub\":\"eve\",\"aud\":\"app\",\"exp\":9999999999}";
    let forged = parts[0] + "." + b64(forged_payload.to_bytes()) + "." + parts[2];
    print("forged: " + err_of(tokens.verify_id_token(p, "app", forged, "", now_s * 1000)));
    // Refresh (sin nonce), userinfo y logout.
    let r = login.refresh_at(p, c, t.refresh_token, now_s * 1000).unwrap();
    print(
        "refreshed: " + r.claims.sub + " keeps refresh=" + to_string(r.refresh_token == "R3FRESH")
    );
    let ui = login.userinfo(p, r.access_token).unwrap();
    print("userinfo: " + json.get_string(ui, "preferred_username").unwrap_or("?"));
    print(
        "logout: "
            + to_string(login.logout_url(p, t.id_token, "http://127.0.0.1:1/")
                .unwrap()
                .starts_with(issuer + "/logout?"))
    );
    // Cliente con secreto equivocado.
    let wrong = login.Client {
        client_id: "app",
        client_secret: "nope",
        redirect_uri: c.redirect_uri,
        scope: c.scope
    };
    print("wrong secret: " + err_of(login.refresh_at(p, wrong, "R3FRESH", now_s * 1000)));
    send(stop, 1);
    0
}
"#).unwrap();
    let want = "discover: true\nauthorize accepted: true\nsigned in: ada ada@example.test refresh=R3FRESH expires_in=300\nstate mismatch: state mismatch (not the sign-in this browser started)\ndenied: the provider answered access_denied: user said no\naccess token: ada client=app scopes=3\nmissing scope: missing scope admin\nid token as access token: not an access token (typ JWT)\nexpired: token expired\nother audience: ID token for another audience\nforged: jose: invalid signature\nrefreshed: ada keeps refresh=true\nuserinfo: ada\nlogout: true\nwrong secret: token endpoint: invalid_client\n";
    let ray = env!("CARGO_BIN_EXE_ray");
    let out = Command::new(ray).args(["run", "prog.ray"]).current_dir(&d).output().unwrap();
    assert!(out.status.success(), "vm: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), want, "vm");
    if has_rustc() {
        let bin: PathBuf = d.join("prog_bin");
        let b = Command::new(ray).args(["build", "prog.ray", "--native", "--no-stubs", "-o", bin.to_str().unwrap()]).current_dir(&d).output().unwrap();
        assert!(b.status.success(), "build --native: {}", String::from_utf8_lossy(&b.stderr));
        let out = Command::new(&bin).current_dir(&d).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), want, "nativo\n{}", String::from_utf8_lossy(&out.stderr));
    }
}
