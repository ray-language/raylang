//! IDEAS §102 (M347) — los huecos de API que rayauth documentó, cada uno con su programa mínimo:
//! stdlib (`std/crypto` HMAC y clave pública, `std/pem`, `std/qr`, `std/json`) en los tres motores,
//! y los paquetes `net`/`web`/`db` desde un PROYECTO CONSUMIDOR con path-deps al repo.

use std::path::PathBuf;
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("raylang_test_rayauth_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Un proyecto consumidor con `net`, `web` y `db` del repo como dependencias por ruta.
fn project(name: &str) -> PathBuf {
    let d = tmp(name);
    let root = env!("CARGO_MANIFEST_DIR");
    std::fs::write(
        d.join("ray.toml"),
        format!(
            "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nentry = \"prog.ray\"\n\n[dependencies]\nnet = \"path:{root}/packages/net\"\nweb = \"path:{root}/packages/web\"\ndb = \"path:{root}/packages/db\"\n"
        ),
    )
    .unwrap();
    d
}

fn ray(dir: &PathBuf, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_ray")).args(args).current_dir(dir).output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned(), out.status.code().unwrap_or(-1))
}

fn has_rustc() -> bool {
    Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// `prog.ray` en VM, intérprete y (con rustc) nativo; los tres deben imprimir `want`.
fn three_engines(dir: &PathBuf, want: &str) {
    for engine in [&["run", "prog.ray"][..], &["run", "--interp", "prog.ray"][..]] {
        let (out, err, code) = ray(dir, engine);
        assert_eq!(code, 0, "{engine:?}: {err}");
        assert_eq!(out, want, "{engine:?}");
    }
    native(dir, want);
}

/// Solo VM y nativo (programas con fibras: el intérprete no las ejecuta). La salida se compara sin las
/// líneas `listening on port N` (puerto efímero).
fn vm_and_native(dir: &PathBuf, want: &str) {
    let strip = |s: String| s.lines().filter(|l| !l.starts_with("listening on port")).map(|l| format!("{l}\n")).collect::<String>();
    let (out, err, code) = ray(dir, &["run", "prog.ray"]);
    assert_eq!(code, 0, "vm: {err}");
    assert_eq!(strip(out), want, "vm");
    if has_rustc() {
        let bin = dir.join("prog_bin");
        let (_o, err, code) = ray(dir, &["build", "prog.ray", "--native", "--no-stubs", "-o", bin.to_str().unwrap()]);
        assert_eq!(code, 0, "build --native: {err}");
        let out = Command::new(&bin).current_dir(dir).output().unwrap();
        assert_eq!(strip(String::from_utf8_lossy(&out.stdout).into_owned()), want, "nativo");
    }
}

fn native(dir: &PathBuf, want: &str) {
    if !has_rustc() {
        return;
    }
    let bin = dir.join("prog_bin");
    let (_o, err, code) = ray(dir, &["build", "prog.ray", "--native", "--no-stubs", "-o", bin.to_str().unwrap()]);
    assert_eq!(code, 0, "build --native: {err}");
    let out = Command::new(&bin).current_dir(dir).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), want, "nativo\n{}", String::from_utf8_lossy(&out.stderr));
}

/// R3: HMAC-SHA1/384/512 y el `hmac(alg, …)` genérico. Vectores de RFC 2202 / RFC 4231 (caso 2:
/// clave "Jefe", mensaje "what do ya want for nothing?").
#[test]
fn hmac_sha1_and_sha512_match_the_rfc_vectors_on_all_engines() {
    let d = tmp("hmac");
    std::fs::write(
        d.join("prog.ray"),
        r#"import std/crypto;
import std/hex;
fn main() -> int {
    let k = "Jefe".to_bytes();
    let m = "what do ya want for nothing?".to_bytes();
    print(hex.hex_encode(crypto.hmac_sha1(k, m)));
    print(hex.hex_encode(crypto.hmac_sha512(k, m)));
    print(hex.hex_encode(crypto.hmac("sha256", k, m).unwrap()) == hex.hex_encode(crypto.hmac_sha256(k, m)));
    print(crypto.hmac("md5", k, m).is_none());
    0
}
"#,
    )
    .unwrap();
    three_engines(
        &d,
        "effcdf6ae5eb2fa2d27416d5f184df9c259a7c79\n164b7a7bfcf819e2e395fbe73b56e0a387bd64222e831fd610270cd7ea2505549758bf75c05a994a6d034f65f8f0e6fdcaeab1a34d4a6b4b636e070a38bce737\ntrue\ntrue\n",
    );
}

/// R18: `[T]` es `ToJson` cuando `T` lo es (y el método manglado `[]#to_json` compila en nativo).
#[test]
fn arrays_of_tojson_values_are_tojson_on_all_engines() {
    let d = tmp("tojson_arrays");
    std::fs::write(
        d.join("prog.ray"),
        r#"import std/json;
fn main() -> int {
    let amr: [string] = ["pwd", "otp"];
    print(json.obj().field("amr", amr).field("ids", [1, 2, 3]).field("ok", [true]).render());
    print(json.list([[1, 2], [3]]).to_json());
    0
}
"#,
    )
    .unwrap();
    three_engines(&d, "{\"amr\":[\"pwd\",\"otp\"],\"ids\":[1,2,3],\"ok\":[true]}\n[[1,2],[3]]\n");
}

/// R1: ECDSA P-256 (una firma DER de OpenSSL se verifica; las propias van y vuelven) y RSA (firma
/// RS256/PS256 con una clave PKCS#8, `rsa_public_key(n, e)` ↔ `rsa_public_components`), más `std/pem`.
/// ring no genera claves RSA, así que la trae el test (`tests/fixtures/rsa_test_key.pem`, 2048 bits
/// de `openssl genpkey`, solo para pruebas).
#[test]
fn p256_and_rsa_sign_and_verify_on_all_engines() {
    let d = tmp("pk");
    let rsa_pem = include_str!("fixtures/rsa_test_key.pem");
    std::fs::write(d.join("rsa.pem"), rsa_pem).unwrap();
    std::fs::write(
        d.join("prog.ray"),
        r#"import std/crypto;
import std/pem;
import std/fs;
import std/hex;
fn main() -> int {
    let msg = "hello rayauth".to_bytes();
    // OpenSSL: `openssl dgst -sha256 -sign ec.pem` (firma DER) y su clave pública SEC1.
    let ec_pub = hex.hex_decode("04dfe55ff2f75500b0b2d663fd2becfc6dce5ad08acccf7bca7bb02828a455dca9eb7dd5a227a3f873602db0a1f8397fd2bf357294bae9e735346f1f43607919e3").unwrap();
    let ec_sig = hex.hex_decode("3045022032325d8bcdcd7ca6705db95718fa6cdfaa20a017ed2d128ab4d04fb567d5933a022100d46f39fab7785ddf80162d5d8bdfa228990b30bea388cb44a048c286fced74fe").unwrap();
    print(crypto.p256_verify_asn1(ec_pub, msg, ec_sig));
    print(crypto.p256_verify_asn1(ec_pub, "x".to_bytes(), ec_sig));
    let k = crypto.p256_generate().unwrap();
    let kp = crypto.p256_public_key(k).unwrap();
    let s = crypto.p256_sign(k, msg).unwrap();
    print("${kp.len()} ${s.len()} ${crypto.p256_verify(kp, msg, s)} ${crypto.p256_verify(kp, msg, b"\x00" + s.sub_bytes(1, 64))}");
    let rsa = pem.decode_label(fs.read_file("rsa.pem").unwrap(), "PRIVATE KEY").unwrap();
    let pk = crypto.rsa_public_key_of(rsa).unwrap();
    let sig = crypto.rsa_pkcs1_sign(rsa, msg).unwrap();
    print("${sig.len()} ${crypto.rsa_pkcs1_verify(pk, msg, sig)} ${crypto.rsa_pkcs1_verify(pk, "t".to_bytes(), sig)}");
    print(crypto.rsa_pss_verify(pk, msg, crypto.rsa_pss_sign(rsa, msg).unwrap()));
    let (n, e) = crypto.rsa_public_components(pk).unwrap();
    print("${n.len()} ${hex.hex_encode(e)} ${crypto.rsa_public_key(n, e) == pk}");
    print(pem.decode(pem.encode("TEST", "abc".to_bytes())).unwrap().label);
    print(crypto.p256_public_key(b"junk"));
    0
}
"#,
    )
    .unwrap();
    three_engines(
        &d,
        "true\nfalse\n65 64 true false\n256 true false\ntrue\n256 010001 true\nTEST\nResult.Err(p256: invalid PKCS#8 key (InvalidEncoding))\n",
    );
}

/// R23: `std/qr` — matriz determinista (misma salida en los tres motores), texto, SVG y PNG
/// (que `std/image` vuelve a leer) y los errores de nivel y de capacidad.
#[test]
fn qr_codes_render_identically_on_all_engines() {
    let d = tmp("qr");
    std::fs::write(
        d.join("prog.ray"),
        r#"import std/qr;
import std/image;
import std/crypto;
import std/hex;
fn main() -> int {
    let code = qr.encode("otpauth://totp/rayauth:alice?secret=JBSWY3DPEHPK3PXP&issuer=rayauth").unwrap();
    print("${code.size} ${code.dark(0, 0)} ${code.dark(7, 0)}");
    let text = code.to_text();
    print(text.split("\n").len());
    print(hex.hex_encode(crypto.sha256(text.to_bytes())).substring(0, 16));
    let svg = code.to_svg(4, 2);
    print(svg.starts_with("<svg") && svg.contains("viewBox=\"0 0 164 164\""));
    let png = code.to_png(3, 1).unwrap();
    let img = image.decode_png(png).unwrap();
    print("${img.width}x${img.height} ${hex.hex_encode(crypto.sha256(png)).substring(0, 16)}");
    print(qr.encode_level("x", "Z"));
    print(qr.encode("a".repeat(5000)).is_err());
    0
}
"#,
    )
    .unwrap();
    let (out, err, code) = ray(&d, &["run", "prog.ray"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.starts_with("37 true false\n21\n"), "{out}");
    assert!(out.contains("\ntrue\n117x117 "), "{out}");
    assert!(out.ends_with("Result.Err(qr: unknown error correction level 'Z' (use L, M, Q or H))\ntrue\n"), "{out}");
    // Determinismo entre motores: la misma salida byte a byte.
    three_engines(&d, &out);
}

/// R4 + R22: JWT con `kid` (HS256 y EdDSA), lectura de cabecera/`kid` sin verificar,
/// `jwt_check_claims` (iss, aud string o lista) y la cookie con `Domain`.
#[test]
fn jwt_kid_claims_and_cookie_domain_on_all_engines() {
    let d = project("jwt_cookie");
    std::fs::write(
        d.join("prog.ray"),
        r#"import net/jwt;
import net/jwt_eddsa;
import net/cookie;
import std/crypto;
fn main() -> int {
    let t = jwt.jwt_sign_kid("s3cret".to_bytes(), "k1", "{\"iss\":\"https://idp\",\"aud\":[\"app\",\"api\"],\"sub\":\"u1\"}");
    print(jwt.jwt_header(t).unwrap());
    print(jwt.jwt_kid(t).unwrap_or("-"));
    let payload = jwt.jwt_verify("s3cret".to_bytes(), t).unwrap();
    print(jwt.jwt_check_claims(payload, "https://idp", "api").is_ok());
    print(jwt.jwt_check_claims(payload, "https://idp", "other"));
    print(jwt.jwt_check_claims(payload, "https://evil", ""));
    print(jwt.jwt_check_claims("{\"aud\":\"app\"}", "", "app").is_ok());
    let seed = crypto.sha256("seed".to_bytes());
    let e = jwt_eddsa.jwt_eddsa_sign_kid(seed, "ed-2026", "{\"aud\":\"app\"}");
    print(jwt.jwt_kid(e).unwrap_or("-"));
    print(jwt_eddsa.jwt_eddsa_verify(crypto.ed25519_public_key(seed).unwrap(), e).unwrap());
    print(jwt.jwt_kid("not.a.jwt").unwrap_or("-"));
    print(cookie.cookie("sid", "abc").with_domain("example.com").with_path("/").with_http_only().set_cookie());
    0
}
"#,
    )
    .unwrap();
    three_engines(
        &d,
        "{\"alg\":\"HS256\",\"typ\":\"JWT\",\"kid\":\"k1\"}\nk1\ntrue\nResult.Err(audience mismatch)\nResult.Err(wrong issuer)\ntrue\ned-2026\n{\"aud\":\"app\"}\n-\nsid=abc; Domain=example.com; Path=/; HttpOnly\n",
    );
}

/// R14: dos `Set-Cookie` en el cable llegan ambos (`raw_headers`, `header_all`, `set_cookies`);
/// `headers` (Map) conserva el último.
#[test]
fn repeated_response_headers_survive_the_wire() {
    let d = project("header_all");
    std::fs::write(
        d.join("prog.ray"),
        r#"import net/webserver;
import net/http;
import std/net;
fn handler(req: webserver.Request) -> webserver.Response {
    var r = webserver.text(200, "hi");
    r.set_cookie = ["a=1; Path=/", "b=2; Path=/; HttpOnly"];
    r
}
fn main() -> int {
    let l = net.tcp_listen("127.0.0.1", 0).unwrap();
    let port = net.local_port(l);
    spawn(fn() { let _ = webserver.serve_on(l, handler); });
    let r = http.fetch("http://127.0.0.1:${port}/").unwrap();
    print(r.header("set-cookie").unwrap_or("-"));
    print(r.header_all("Set-Cookie"));
    print(r.set_cookies().len());
    print(r.header_all("x-none").len());
    0
}
"#,
    )
    .unwrap();
    vm_and_native(&d, "b=2; Path=/; HttpOnly\n[a=1; Path=/, b=2; Path=/; HttpOnly]\n2\n0\n");
}

/// R24: `listen_with` combina límites y drenaje (y TLS) en una llamada; `with_*` resuelven por el
/// tipo sin importar `webserver`.
#[test]
fn listen_with_serves_with_limits_and_drain() {
    let d = project("listen_with");
    std::fs::write(
        d.join("prog.ray"),
        r#"import web/framework;
from web/framework import App;
import net/webserver;
import net/http;
import std/net;
import std/time;
fn routes() -> App {
    var app = framework.new_app();
    app.GET("/", fn(c: framework.Ctx, r: framework.Res) { r.text("hello"); });
    app
}
fn main() -> int {
    let l = net.tcp_listen("127.0.0.1", 0).unwrap();
    let port = net.local_port(l);
    close(l);
    var lim = webserver.default_limits();
    lim.max_body_bytes = 4096;
    let opts = framework.options().with_limits(lim).with_drain(2000);
    spawn(fn() { let _ = framework.listen_with(fn() -> App { routes() }, "127.0.0.1", port, opts); });
    time.sleep(300);
    let r = http.fetch("http://127.0.0.1:${port}/").unwrap();
    print(r.status);
    print(http.body_text(r).unwrap_or("?"));
    0
}
"#,
    )
    .unwrap();
    vm_and_native(&d, "200\nhello\n");
}

/// R6: `sqlite.query_opt` distingue `NULL` (`None`) de `""`.
#[test]
fn sqlite_query_opt_tells_null_from_empty_on_all_engines() {
    let d = project("sqlite_null");
    std::fs::write(
        d.join("prog.ray"),
        r#"import db/sqlite;
fn main() -> int {
    let c = sqlite.connect(":memory:").unwrap();
    let _ = sqlite.exec(c, "CREATE TABLE t (a TEXT, b INTEGER)", []).unwrap();
    let _ = sqlite.exec(c, "INSERT INTO t VALUES (NULL, 1), ('', 2), ('x', NULL)", []).unwrap();
    print(sqlite.query(c, "SELECT a, b FROM t ORDER BY rowid", []).unwrap());
    for row in sqlite.query_opt(c, "SELECT a, b FROM t ORDER BY rowid", []).unwrap() {
        var cells: [string] = [];
        for cell in row { cells.push(match (cell) { Option.Some(v) => "'" + v + "'", Option.None => "NULL", }); }
        print(join(cells, " | "));
    }
    print(sqlite.query_opt(c, "SELECT * FROM nope", []).is_err());
    0
}
"#,
    )
    .unwrap();
    three_engines(&d, "[[, 1], [, 2], [x, ]]\nNULL | '1'\n'' | '2'\n'x' | NULL\ntrue\n");
}

/// M348 (IDEAS §102 R1, la parte abierta): `rsa_generate` produce PKCS#8 que ring importa, firma y
/// verifica; `std/pem` lo escribe y, si hay `openssl` en el PATH, OpenSSL valida la clave y la firma.
#[test]
fn rsa_generate_produces_a_key_ring_and_openssl_accept() {
    let d = tmp("rsa_generate");
    std::fs::write(
        d.join("prog.ray"),
        r#"import std/crypto;
import std/pem;
import std/fs;
fn main() -> int {
    let k = crypto.rsa_generate(2048).unwrap();
    fs.write_file("gen.pem", pem.encode("PRIVATE KEY", k)).unwrap();
    let msg = "hello".to_bytes();
    let sig = crypto.rsa_pkcs1_sign(k, msg).unwrap();
    fs.write_file_bytes("gen_sig.bin", sig).unwrap();
    let pk = crypto.rsa_public_key_of(k).unwrap();
    print(crypto.rsa_pkcs1_verify(pk, msg, sig));
    let (n, e) = crypto.rsa_public_components(pk).unwrap();
    print("${n.len() * 8} ${e.len()}");
    print(crypto.rsa_generate(1000));
    0
}
"#,
    )
    .unwrap();
    three_engines(&d, "true\n2048 3\nResult.Err(rsa_generate: bits must be a multiple of 64 between 2048 and 4096 (got 1000))\n");
    if Command::new("openssl").arg("version").output().map(|o| o.status.success()).unwrap_or(false) {
        let check = Command::new("openssl").args(["pkey", "-in", "gen.pem", "-check", "-noout"]).current_dir(&d).output().unwrap();
        assert!(check.status.success(), "openssl pkey -check: {}", String::from_utf8_lossy(&check.stderr));
        std::fs::write(d.join("m.txt"), "hello").unwrap();
        let pubout = Command::new("openssl").args(["pkey", "-in", "gen.pem", "-pubout", "-out", "gen_pub.pem"]).current_dir(&d).output().unwrap();
        assert!(pubout.status.success());
        let verify = Command::new("openssl")
            .args(["dgst", "-sha256", "-verify", "gen_pub.pem", "-signature", "gen_sig.bin", "m.txt"])
            .current_dir(&d)
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&verify.stdout).contains("Verified OK"), "{}", String::from_utf8_lossy(&verify.stderr));
    }
}

/// M349 (IDEAS §102 R15): `std/template` con objetos — `VMap`, rutas con punto en `{{ }}`, `{% if %}`
/// y `{% for %}`, índices en listas (`items.0`), `from_json`. Misma salida en los tres motores.
#[test]
fn templates_render_objects_with_dotted_paths_on_all_engines() {
    let d = tmp("template_objects");
    std::fs::write(
        d.join("prog.ray"),
        r#"import std/template;
import std/json;
from std/template import val_str, val_int, val_bool, val_map, val_list, field;
fn main() -> int {
    let users = [
        val_map([field("name", val_str("Ada <3")), field("admin", val_bool(true)), field("roles", val_list([val_str("ops"), val_str("dev")]))]),
        val_map([field("name", val_str("Bob")), field("admin", val_bool(false)), field("roles", val_list([]))]),
    ];
    let tpl = "{% for u in users %}<tr><td>{{ u.name }}</td><td>{% if u.admin %}yes{% else %}no{% endif %}</td><td>{% for r in u.roles %}[{{ r }}]{% endfor %}</td></tr>\n{% endfor %}first={{ users.0.name }} missing={{ users.5.name }}|{{ site.title }}|{{ site.owner.email }}";
    let ctx = [template.ctx_list("users", users), template.ctx_map("site", [field("title", val_str("rayauth")), field("owner", val_map([field("email", val_str("a@b.c"))]))])];
    print(template.render_template(tpl, ctx).unwrap());
    let j = json.parse("{\"items\":[{\"id\":1,\"price\":2.5,\"tags\":[\"x\"]}],\"n\":null,\"ok\":true}").unwrap();
    print(template.render_template("{% for i in data.items %}{{ i.id }}:{{ i.price }}:{{ i.tags.0 }}{% endfor %} n=[{{ data.n }}] ok={{ data.ok }} all={{& data }}", [template.ctx_val("data", template.from_json(j))]).unwrap());
    0
}
"#,
    )
    .unwrap();
    three_engines(
        &d,
        "<tr><td>Ada &lt;3</td><td>yes</td><td>[ops][dev]</td></tr>\n<tr><td>Bob</td><td>no</td><td></td></tr>\nfirst=Ada &lt;3 missing=|rayauth|a@b.c\n1:2.5:x n=[] ok=true all=items=id=1, price=2.5, tags=x, n=, ok=true\n",
    );
}

/// M350 (IDEAS §102 R9): sesiones persistentes — `web.sessions_with` sobre `db/sessions.sqlite`
/// (la sesión sobrevive a otro servidor sobre la misma base de datos, `session_clear` la cierra) y el
/// backend de memoria con TTL (caduca y `sweep` la barre). VM y nativo.
#[test]
fn sessions_persist_in_sqlite_and_expire_with_a_ttl() {
    let d = project("sessions_sqlite");
    std::fs::write(d.join("prog.ray"), r#"import web/framework;
from web/framework import App;
import net/http;
import net/session_store;
import db/sqlite;
import db/sessions;
import std/net;
import std/time;
fn routes(sess: framework.Sessions) -> App {
    var app = framework.new_app();
    app.GET("/set", fn(c: framework.Ctx, r: framework.Res) { framework.session_put(sess, c, r, "user", "ada"); r.text("ok"); });
    app.GET("/get", fn(c: framework.Ctx, r: framework.Res) { r.text("user=" + framework.session_get(sess, c, r, "user")); });
    app.GET("/out", fn(c: framework.Ctx, r: framework.Res) { framework.session_clear(sess, c, r); r.text("bye"); });
    app
}
fn serve(sess: framework.Sessions) -> int {
    let l = net.tcp_listen("127.0.0.1", 0).unwrap();
    let port = net.local_port(l);
    spawn(fn() { let _ = framework.listen_on(fn() -> App { routes(sess) }, l); });
    port
}
fn cookie_of(r: http.Response) -> string {
    let sc = http.set_cookies(r);
    if (sc.len() == 0) { return ""; }
    sc[0].split(";")[0]
}
fn get(port: int, path: string, cookie: string) -> http.Response {
    var h: Map<string, string> = Map.new();
    if (cookie != "") { h.insert("Cookie", cookie); }
    http.request_with("GET", "http://127.0.0.1:${port}" + path, "", h).unwrap()
}
fn main() -> int {
    let db = sqlite.connect("sess.db").unwrap();
    let store = sessions.sqlite(db, 3600).unwrap();
    let port = serve(framework.sessions_with(store));
    time.sleep(200);
    let r1 = get(port, "/set", "");
    let ck = cookie_of(r1);
    print(ck.starts_with("ray_session=") && ck.len() == 44);
    print(http.body_text(get(port, "/get", ck)).unwrap());
    print(http.body_text(get(port, "/get", "")).unwrap());
    // Persistencia: un segundo servidor sobre la misma base de datos ve la sesión.
    let db2 = sqlite.connect("sess.db").unwrap();
    let port2 = serve(framework.sessions_with(sessions.sqlite(db2, 3600).unwrap()));
    time.sleep(200);
    print(http.body_text(get(port2, "/get", ck)).unwrap());
    print(sqlite.query(db2, "SELECT count(*) FROM ray_sessions", []).unwrap()[0][0]);
    let _ = get(port2, "/out", ck);
    time.sleep(100);
    print(http.body_text(get(port, "/get", ck)).unwrap());
    // Memoria con TTL de 1 s: la sesión caduca y el barrido la borra.
    let mem = session_store.memory("", false, 1).unwrap();
    session_store.set(mem, "s1", "k", "v");
    print(session_store.get(mem, "s1", "k").unwrap_or("-"));
    time.sleep(1200);
    print(session_store.get(mem, "s1", "k").unwrap_or("-"));
    print(session_store.sweep(mem));
    0
}
"#).unwrap();
    vm_and_native(&d, "true
user=ada
user=
user=ada
1
user=
v
-
1
");
    // El nativo arranca con la base de datos que dejó la VM: el programa cierra su sesión al final,
    // y la cuenta de filas sigue valiendo 1 porque cada ejecución escribe una sesión nueva.
}
