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

/// M351 (IDEAS §102 R7): el cliente SMTP de `net/smtp` contra un servidor falso escrito en raylang:
/// plano con AUTH PLAIN, STARTTLS (certificado de pruebas de `tests/fixtures`, confiado vía
/// `SSL_CERT_FILE`), TLS implícito, un servidor sin STARTTLS cuando se exige (error, sin degradar) y
/// un destinatario rechazado. También fija el bug de runtime que destapó: `socket_read`/`socket_write`
/// (string) no sabían leer/escribir un handle TLS tras `tls_upgrade`. VM y nativo.
#[test]
fn smtp_sends_over_plain_starttls_and_tls() {
    let d = project("smtp");
    let root = env!("CARGO_MANIFEST_DIR");
    for f in ["tls_cert.pem", "tls_key.pem", "tls_ca.pem"] {
        std::fs::copy(format!("{root}/tests/fixtures/{f}"), d.join(f)).unwrap();
    }
    std::fs::write(d.join("prog.ray"), r#"import net/smtp;
import net/mail;
import std/net;
import std/fs;
import std/time;
import std/base64;

// Un servidor SMTP falso: `mode` = "plain" | "starttls" | "tls". Registra el diálogo en `log`
// (un canal) y acepta todo salvo RCPT a "reject@…".
fn serve(mode: string, cert: string, key: string, log: Channel<string>) -> int {
    let l = net.tcp_listen("127.0.0.1", 0).unwrap();
    let port = net.local_port(l);
    spawn(fn() {
        let tcp = net.tcp_accept(l).unwrap();
        var c = if (mode == "tls") { net.tls_accept(tcp, cert, key).unwrap() } else { tcp };
        var buf = "";
        let _ = net.socket_write(c, "220 fake.example ESMTP\r\n");
        var open = true;
        var in_data = false;
        var secure = mode == "tls";
        while (open) {
            match (net.socket_read(c)) {
                Result.Ok(chunk) => {
                    if (chunk == "") { open = false; } else { buf = buf + chunk; }
                },
                Result.Err(_) => { open = false; },
            }
            var again = true;
            while (again && open) {
                match (buf.index_of("\r\n")) {
                    Option.Some(i) => {
                        let line = buf.substring(0, i);
                        buf = buf.substring(i + 2, buf.len());
                        if (in_data) {
                            if (line == ".") {
                                in_data = false;
                                let _ = net.socket_write(c, "250 2.0.0 queued as 42\r\n");
                            } else {
                                send(log, "DATA:" + line);
                            }
                        } else {
                            send(log, (if (secure) { "S:" } else { "P:" }) + line);
                            let up = line.to_upper();
                            if (up.starts_with("EHLO")) {
                                let _ = net.socket_write(c, "250-fake.example\r\n250-AUTH PLAIN LOGIN\r\n" + (if (mode == "starttls" && !secure) { "250-STARTTLS\r\n" } else { "" }) + "250 SIZE 10000000\r\n");
                            } else if (up == "STARTTLS") {
                                let _ = net.socket_write(c, "220 go ahead\r\n");
                                c = net.tls_accept(c, cert, key).unwrap();
                                secure = true;
                                buf = "";
                            } else if (up.starts_with("AUTH PLAIN")) {
                                let _ = net.socket_write(c, "235 2.7.0 ok\r\n");
                            } else if (up.starts_with("RCPT TO:<REJECT@")) {
                                let _ = net.socket_write(c, "550 5.1.1 no such user\r\n");
                            } else if (up == "DATA") {
                                in_data = true;
                                let _ = net.socket_write(c, "354 go\r\n");
                            } else if (up == "QUIT") {
                                let _ = net.socket_write(c, "221 bye\r\n");
                                open = false;
                            } else {
                                let _ = net.socket_write(c, "250 ok\r\n");
                            }
                        }
                    },
                    Option.None => { again = false; },
                }
            }
        }
        close(c);
    });
    port
}

fn drain(log: Channel<string>) -> [string] {
    var out: [string] = [];
    var more = true;
    while (more) {
        match (try_recv(log)) {
            Received.Got(l) => out.push(l),
            Received.Empty => more = false,
            Received.Closed => more = false,
        }
    }
    out
}

fn main() -> int {
    let cert = fs.read_file("tls_cert.pem").unwrap();
    let key = fs.read_file("tls_key.pem").unwrap();
    let m = smtp.message(mail.address("Ray Auth", "noreply@rayauth.test"), ["ada@example.com", mail.address("Bob Ñ", "bob@example.com")], "Verifica tu correo — ✓", "Hola Ada,\r\nPulsa el enlace.\r\n.inicio con punto\r\n")
        .with_html("<p>Hola <b>Ada</b></p>")
        .with_reply_to("soporte@rayauth.test")
        .with_header("X-Mailer", "rayauth");
    let rendered = smtp.render(m);
    print(rendered.contains("Subject: =?UTF-8?B?") && rendered.contains("multipart/alternative") && rendered.contains("\r\n..inicio con punto"));
    print(smtp.recipients(m));

    // 1. Plano, con login.
    let log1: Channel<string> = Channel.bounded(256);
    let p1 = serve("plain", cert, key, log1);
    time.sleep(100);
    let srv1 = smtp.server("127.0.0.1", p1).with_security("none").with_login("user", "pass");
    print(smtp.send(srv1, m));
    time.sleep(100);
    let l1 = drain(log1);
    print(l1[0]); print(l1[1]); print(l1[2]); print(l1[3]); print(l1[4]);
    print(l1.contains("P:DATA") && l1.contains("DATA:..inicio con punto") && l1.contains("P:QUIT"));
    print(base64.base64_decode(l1[1].substring(13, l1[1].len())).unwrap() == "\u{0}user\u{0}pass".to_bytes());

    // 2. STARTTLS (el servidor se llama "localhost" en el certificado).
    let log2: Channel<string> = Channel.bounded(256);
    let p2 = serve("starttls", cert, key, log2);
    time.sleep(100);
    print(smtp.send(smtp.server("localhost", p2), m));
    time.sleep(100);
    let l2 = drain(log2);
    print(l2);
    print(l2.contains("S:DATA") && !l2.contains("P:DATA"));

    // 3. TLS implícito.
    let log3: Channel<string> = Channel.bounded(256);
    let p3 = serve("tls", cert, key, log3);
    time.sleep(100);
    print(smtp.send(smtp.server("localhost", p3).with_security("tls"), m));
    time.sleep(100);
    print(drain(log3)[0]);

    // 4. Un servidor plano cuando se exige STARTTLS: error, sin degradar.
    let log4: Channel<string> = Channel.bounded(256);
    let p4 = serve("plain", cert, key, log4);
    time.sleep(100);
    print(smtp.send(smtp.server("127.0.0.1", p4), m));

    // 5. Destinatario rechazado.
    let log5: Channel<string> = Channel.bounded(256);
    let p5 = serve("plain", cert, key, log5);
    time.sleep(100);
    print(smtp.send(smtp.server("127.0.0.1", p5).with_security("none"), smtp.message("a@b.c", ["reject@example.com"], "x", "y")));
    0
}
"#).unwrap();
    let want = "true
[ada@example.com, bob@example.com]
Result.Ok(2.0.0 queued as 42)
P:EHLO localhost
P:AUTH PLAIN AHVzZXIAcGFzcw==
P:MAIL FROM:<noreply@rayauth.test>
P:RCPT TO:<ada@example.com>
P:RCPT TO:<bob@example.com>
true
true
Result.Ok(2.0.0 queued as 42)
[P:EHLO localhost, P:STARTTLS, S:EHLO localhost, S:MAIL FROM:<noreply@rayauth.test>, S:RCPT TO:<ada@example.com>, S:RCPT TO:<bob@example.com>, S:DATA, ";
    let ca = d.join("tls_ca.pem");
    let run = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_ray")).args(args).env("SSL_CERT_FILE", &ca).current_dir(&d).output().unwrap();
        (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned(), out.status.code().unwrap_or(-1))
    };
    let check = |out: &str, label: &str| {
        assert!(out.starts_with(want), "{label}: inicio inesperado:
{out}");
        assert!(out.contains("S:QUIT]
true
Result.Ok(2.0.0 queued as 42)
S:EHLO localhost
Result.Err(smtp: the server does not offer STARTTLS"), "{label}:
{out}");
        assert!(out.ends_with("Result.Err(smtp: RCPT failed: 550 5.1.1 no such user)
"), "{label}:
{out}");
    };
    let (out, err, code) = run(&["run", "prog.ray"]);
    assert_eq!(code, 0, "vm: {err}");
    check(&out, "vm");
    if has_rustc() {
        let bin = d.join("prog_bin");
        let (_o, err, code) = run(&["build", "prog.ray", "--native", "--no-stubs", "-o", bin.to_str().unwrap()]);
        assert_eq!(code, 0, "build --native: {err}");
        let out = Command::new(&bin).env("SSL_CERT_FILE", &ca).current_dir(&d).output().unwrap();
        check(&String::from_utf8_lossy(&out.stdout), "nativo");
    }
}

/// M352: el backend PostgreSQL de las sesiones (`sessions.postgres` y `postgres_pool`) contra un
/// servidor REAL — solo si `RAY_TEST_PGPORT` apunta a uno (usuario `ray`, contraseña `raytest`, base
/// `raytest`: `docker run -e POSTGRES_USER=ray -e POSTGRES_PASSWORD=raytest -e POSTGRES_DB=raytest
/// -p 127.0.0.1:<puerto>:5432 postgres:18`); sin él, se salta. Mismo guion que el de SQLite: dos
/// servidores (conexión y pool) sobre la misma base, logout, TTL y barrido. VM y nativo.
#[test]
fn sessions_persist_in_postgres_when_a_server_is_available() {
    let Ok(port) = std::env::var("RAY_TEST_PGPORT") else {
        eprintln!("saltando: RAY_TEST_PGPORT no definido");
        return;
    };
    let d = project("sessions_pg");
    std::fs::write(d.join("prog.ray"), r#"import web/framework;
from web/framework import App;
import net/http;
import net/session_store;
import db/postgres;
import db/sessions;
import std/net;
import std/time;
import std/crypto;
import std/hex;
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
    let pgport = parse_int(env("PGPORT").unwrap_or("5432")).unwrap_or(5432);
    let nonce = hex.hex_encode(crypto.random_bytes(12));
    let c1 = postgres.connect("127.0.0.1", pgport, "ray", "raytest", "raytest", nonce).unwrap();
    let _ = postgres.exec(c1, "DROP TABLE IF EXISTS ray_sessions", []).unwrap();
    let store = sessions.postgres(c1, 3600).unwrap();
    let port = serve(framework.sessions_with(store));
    time.sleep(200);
    let ck = cookie_of(get(port, "/set", ""));
    print(ck.len() == 44);
    print(http.body_text(get(port, "/get", ck)).unwrap());
    // Segundo servidor sobre un POOL a la misma base: ve la sesión.
    let pool = postgres.pool("127.0.0.1", pgport, "ray", "raytest", "raytest", 4);
    let port2 = serve(framework.sessions_with(sessions.postgres_pool(pool, 3600).unwrap()));
    time.sleep(200);
    print(http.body_text(get(port2, "/get", ck)).unwrap());
    let c2 = postgres.connect("127.0.0.1", pgport, "ray", "raytest", "raytest", nonce + "b").unwrap();
    print(postgres.query(c2, "SELECT count(*), min(expires_at) > 0 FROM ray_sessions", []).unwrap()[0]);
    let _ = get(port2, "/out", ck);
    time.sleep(100);
    print(http.body_text(get(port, "/get", ck)).unwrap());
    print(postgres.query(c2, "SELECT count(*) FROM ray_sessions", []).unwrap()[0][0]);
    // TTL de 1 s sobre Postgres: caduca y el barrido borra.
    let st = sessions.postgres(c2, 1).unwrap();
    session_store.set(st, "s1", "k", "v");
    print(session_store.get(st, "s1", "k").unwrap_or("-"));
    time.sleep(1200);
    print(session_store.get(st, "s1", "k").unwrap_or("-"));
    print(session_store.sweep(st));
    0
}
"#).unwrap();
    let want = "true\nuser=ada\nuser=ada\n[1, t]\nuser=\n0\nv\n-\n1\n";
    let strip = |s: String| s.lines().filter(|l| !l.starts_with("listening on port")).map(|l| format!("{l}\n")).collect::<String>();
    let out = Command::new(env!("CARGO_BIN_EXE_ray")).args(["run", "prog.ray"]).env("PGPORT", &port).current_dir(&d).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "vm: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(strip(String::from_utf8_lossy(&out.stdout).into_owned()), want, "vm");
    if has_rustc() {
        let bin = d.join("prog_bin");
        let (_o, err, code) = ray(&d, &["build", "prog.ray", "--native", "--no-stubs", "-o", bin.to_str().unwrap()]);
        assert_eq!(code, 0, "build --native: {err}");
        let out = Command::new(&bin).env("PGPORT", &port).current_dir(&d).output().unwrap();
        assert_eq!(strip(String::from_utf8_lossy(&out.stdout).into_owned()), want, "nativo");
    }
}

/// M353 (IDEAS §102 R8): `std/xml` — parse (prolog, comentarios, PI, namespaces resueltos, entidades
/// predefinidas y numéricas, CDATA, contenido mixto), consulta (`child`, `children_named`, `find`,
/// `find_all`, `find_ns`, `attr`, `text`, `text_of`), construcción y serialización (`serialize`,
/// `serialize_doc`, `pretty`, `escape`), ida y vuelta, y los errores con línea (cierre descuadrado,
/// DOCTYPE rechazado, entidad desconocida, dos raíces). Misma salida en los tres motores.
#[test]
fn xml_parses_queries_and_serializes_on_all_engines() {
    let d = tmp("xml");
    std::fs::write(d.join("prog.ray"), r#"import std/xml;
fn main() -> int {
    let src = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!-- a feed -->\n<feed xmlns=\"http://www.w3.org/2005/Atom\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\n  <title type=\"text\">Ray &amp; friends &#x2713; &#169;</title>\n  <entry>\n    <title>First</title>\n    <dc:creator>Ada</dc:creator>\n    <link href=\"https://x.y/1?a=1&amp;b=2\" rel='alternate'/>\n    <content><![CDATA[<b>raw</b> & more]]></content>\n  </entry>\n  <entry><title>Second</title><summary>mixed <em>text</em> here</summary></entry>\n  <?php echo 1 ?>\n</feed>";
    let doc = xml.parse(src).unwrap();
    print("${doc.name} ns=${doc.ns} children=${doc.elements().len()}");
    print(doc.text_of("title"));
    print(doc.child("title").unwrap().attr("type").unwrap_or("-"));
    let entries = doc.children_named("entry");
    print(entries.len());
    print(entries[0].text_of("creator") + " / " + entries[0].text_of("dc:creator"));
    print(entries[0].find("link").unwrap().attr("href").unwrap_or("-"));
    print(entries[0].text_of("content"));
    print(xml.find_ns(doc, "http://purl.org/dc/elements/1.1/", "creator").len());
    print(xml.find_all(doc, "title").len());
    print(xml.serialize(entries[1]));
    print(xml.pretty(entries[0], 2));
    let built = xml.element("rpc").with_attr("v", "1 < 2 \"q\"").with_child(xml.element("name").with_text("a & b")).with_node(xml.Node.CData("<x/>"));
    print(xml.serialize_doc(built));
    print(xml.parse(xml.serialize(built)).unwrap().text_of("name"));
    print(xml.parse("<a><b></a>"));
    print(xml.parse("<!DOCTYPE foo [<!ENTITY x SYSTEM \"file:///etc/passwd\">]><a>&x;</a>"));
    print(xml.parse("<a>&nope;</a>"));
    print(xml.parse("<a>x</a><b/>"));
    print(xml.unescape("&lt;p&gt; &#65;&#x42;"));
    0
}
"#).unwrap();
    three_engines(&d, r#"feed ns=http://www.w3.org/2005/Atom children=3
Ray & friends ✓ ©
text
2
Ada / Ada
https://x.y/1?a=1&b=2
<b>raw</b> & more
1
3
<entry><title>Second</title><summary>mixed <em>text</em> here</summary></entry>
<entry>
  <title>First</title>
  <dc:creator>Ada</dc:creator>
  <link href="https://x.y/1?a=1&amp;b=2" rel="alternate"/>
  <content><![CDATA[<b>raw</b> & more]]></content>
</entry>

<?xml version="1.0" encoding="UTF-8"?>
<rpc v="1 &lt; 2 &quot;q&quot;"><name>a &amp; b</name><![CDATA[<x/>]]></rpc>
a & b
Result.Err(xml: line 1: mismatched closing tag </a> for <b>)
Result.Err(xml: line 1: DOCTYPE is not supported (no DTD or external entities, by design))
Result.Err(xml: unknown entity '&nope;' (no DTD support))
Result.Err(xml: line 1: more than one root element)
Result.Ok(<p> AB)
"#);
}
