//! rayreq #251: `std/url` trocea (`parse`), recompone (`render`) y resuelve referencias
//! relativas (`resolve`) según la RFC 3986, con los ejemplos normales y anómalos de su §5.4, y
//! `parse_query_pairs` conserva orden y claves repetidas. VM e intérprete (el nativo lo cubre
//! el corpus de la stdlib).

use std::process::Command;

const PROG: &str = r##"import std/url;

fn check(r: string, want: string) -> int {
    let got = match (url.resolve("http://a/b/c/d;p?q", r)) {
        Result.Ok(s) => s,
        Result.Err(e) => "ERR " + e,
    };
    if (got != want) {
        print("FAIL " + r + " -> " + got + " (want " + want + ")");
        return 1;
    }
    0
}

fn show(s: string) -> string {
    match (url.parse(s)) {
        Result.Ok(u) => {
            let h = match (u.host) { Option.Some(x) => "[" + x + "]", Option.None => "-" };
            let p = match (u.port) { Option.Some(x) => to_string(x), Option.None => "-" };
            let q = match (u.query) { Option.Some(x) => "?" + x, Option.None => "-" };
            let f = match (u.fragment) { Option.Some(x) => "#" + x, Option.None => "-" };
            let ui = match (u.userinfo) { Option.Some(x) => x + "@", Option.None => "-" };
            u.scheme + " " + ui + " " + h + " " + p + " " + u.path + " " + q + " " + f + " | " + url.render(u)
        },
        Result.Err(e) => "ERR " + e,
    }
}

fn main() -> int {
    var bad = 0;
    // RFC 3986 §5.4.1 normales
    bad = bad + check("g:h", "g:h");
    bad = bad + check("g", "http://a/b/c/g");
    bad = bad + check("./g", "http://a/b/c/g");
    bad = bad + check("g/", "http://a/b/c/g/");
    bad = bad + check("/g", "http://a/g");
    bad = bad + check("//g", "http://g");
    bad = bad + check("?y", "http://a/b/c/d;p?y");
    bad = bad + check("g?y", "http://a/b/c/g?y");
    bad = bad + check("#s", "http://a/b/c/d;p?q#s");
    bad = bad + check("g#s", "http://a/b/c/g#s");
    bad = bad + check("g?y#s", "http://a/b/c/g?y#s");
    bad = bad + check(";x", "http://a/b/c/;x");
    bad = bad + check("g;x", "http://a/b/c/g;x");
    bad = bad + check("g;x?y#s", "http://a/b/c/g;x?y#s");
    bad = bad + check("", "http://a/b/c/d;p?q");
    bad = bad + check(".", "http://a/b/c/");
    bad = bad + check("./", "http://a/b/c/");
    bad = bad + check("..", "http://a/b/");
    bad = bad + check("../", "http://a/b/");
    bad = bad + check("../g", "http://a/b/g");
    bad = bad + check("../..", "http://a/");
    bad = bad + check("../../", "http://a/");
    bad = bad + check("../../g", "http://a/g");
    // §5.4.2 anómalos
    bad = bad + check("../../../g", "http://a/g");
    bad = bad + check("../../../../g", "http://a/g");
    bad = bad + check("/./g", "http://a/g");
    bad = bad + check("/../g", "http://a/g");
    bad = bad + check("g.", "http://a/b/c/g.");
    bad = bad + check(".g", "http://a/b/c/.g");
    bad = bad + check("g..", "http://a/b/c/g..");
    bad = bad + check("..g", "http://a/b/c/..g");
    bad = bad + check("./../g", "http://a/b/g");
    bad = bad + check("./g/.", "http://a/b/c/g/");
    bad = bad + check("g/./h", "http://a/b/c/g/h");
    bad = bad + check("g/../h", "http://a/b/c/h");
    bad = bad + check("g;x=1/./y", "http://a/b/c/g;x=1/y");
    bad = bad + check("g;x=1/../y", "http://a/b/c/y");
    bad = bad + check("g?y/./x", "http://a/b/c/g?y/./x");
    bad = bad + check("g?y/../x", "http://a/b/c/g?y/../x");
    bad = bad + check("g#s/./x", "http://a/b/c/g#s/./x");
    bad = bad + check("g#s/../x", "http://a/b/c/g#s/../x");
    bad = bad + check("http:g", "http:g");
    print("rfc failures: ${bad}");
    print(show("https://user:pw@Example.com:8443/a/b?x=1&y#frag"));
    print(show("http://[::1]:8080/"));
    print(show("file:///etc/hosts"));
    print(show("mailto:a@b.c"));
    print(show("/rel/path?q"));
    print(show("http://h?"));
    print(show("http://h:/p"));
    print(show("http://h:99999/"));
    print(show("http://[::1/"));
    print(show("1abc:x"));
    print(show("http://a b/"));
    let ps = url.parse_query_pairs("a=1&b=x%20y&a=2&c&d=+");
    for kv in ps { print(kv.0 + "=" + kv.1 + ";"); }
    print(url.build_query_pairs([("z", "1"), ("a", "b c"), ("z", "2")]));
    0
}
"##;

const WANT: &str = r##"rfc failures: 0
https user:pw@ [Example.com] 8443 /a/b ?x=1&y #frag | https://user:pw@Example.com:8443/a/b?x=1&y#frag
http - [::1] 8080 / - - | http://[::1]:8080/
file - [] - /etc/hosts - - | file:///etc/hosts
mailto - - - a@b.c - - | mailto:a@b.c
 - - - /rel/path ?q - | /rel/path?q
http - [h] -  ? - | http://h?
http - [h] - /p - - | http://h/p
ERR invalid URL: port out of range: 99999
ERR invalid URL: unclosed '[' in the host
ERR invalid URL: invalid scheme: 1abc
ERR invalid URL (control character or space; percent-encode it)
a=1;
b=x y;
a=2;
c=;
d= ;
z=1&a=b%20c&z=2
"##;

#[test]
fn std_url_parses_renders_and_resolves_per_rfc_3986() {
    let dir = std::env::temp_dir().join(format!("ray_std_url_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.ray"), PROG).unwrap();
    for flags in [&[][..], &["--interp"][..]] {
        let out = Command::new(env!("CARGO_BIN_EXE_ray")).arg("run").args(flags).arg("a.ray").current_dir(&dir).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "{flags:?}: {}", String::from_utf8_lossy(&out.stderr));
    }
}
