//! rayreq #259 + #257: `json.Doc` conserva el orden de los miembros (y las claves repetidas) y el
//! texto de cada número; `parse_doc`/`render_doc` hacen la ida y vuelta exacta, `doc_int` lee 64
//! bits exactos y `doc_pointer` sigue un JSON Pointer. De paso, `parse` y `parse_doc` validan la
//! gramática de los números (RFC 8259 §6). VM e intérprete.

use std::process::Command;

const PROG: &str = r##"import std/json;
from std/json import Doc, Member;

fn show(o: Option<string>) -> string { o.unwrap_or("None") }
fn showi(o: Option<int>) -> string { match (o) { Option.Some(n) => to_string(n), Option.None => "None" } }

fn main() {
    let text = "{\"z\": 1, \"a\": {\"id\": 9007199254740993, \"price\": 1.50, \"e\": 1e3}, \"list\": [true, null, \"x\\ny\"], \"z\": 2, \"/k~\": 7}";
    let d = json.parse_doc(text).unwrap();
    print(json.render_doc(d));
    print(json.render_doc_pretty(d, 2).replace("\n", "|"));
    var keys: [string] = [];
    for m in json.doc_members(d) { keys.push(m.key); }
    print("keys " + keys.join(","));
    print("z " + show(json.doc_text(json.doc_get(d, "z").unwrap())));
    print("id " + showi(json.doc_int(json.doc_pointer(d, "/a/id").unwrap())));
    print("price " + show(json.doc_text(json.doc_pointer(d, "/a/price").unwrap())) + " int=" + showi(json.doc_int(json.doc_pointer(d, "/a/price").unwrap())));
    print("e int " + showi(json.doc_int(json.doc_pointer(d, "/a/e").unwrap())));
    print("list1 " + json.render_doc(json.doc_pointer(d, "/list/1").unwrap()) + " esc " + show(json.doc_text(json.doc_pointer(d, "/list/2").unwrap())).replace("\n", "\\n"));
    print("ptr esc " + json.render_doc(json.doc_pointer(d, "/~1k~0").unwrap()));
    print("missing " + match (json.doc_pointer(d, "/a/nope")) { Option.Some(_) => "some", Option.None => "None" });
    print("json_of " + json.stringify(json.json_of(d)));
    print("doc_of " + json.render_doc(json.doc_of(json.parse("{\"b\":2,\"a\":[1.5,true]}").unwrap())));
    let built = Doc.Obj([Member { key: "name", value: Doc.Str("ada") }, Member { key: "id", value: Doc.Num("12345678901234567890") }]);
    print("built " + json.obj().field("d", built).render());
    for bad in ["+1", "01", "1.", ".5", "-", "1e", "[1,]", "{\"a\" 1}"] {
        print("bad " + bad + " doc=" + match (json.parse_doc(bad)) { Result.Ok(_) => "ok", Result.Err(e) => e } + " json=" + match (json.parse(bad)) { Result.Ok(_) => "ok", Result.Err(e) => e });
    }
    print("relaxed " + json.render_doc(json.parse_doc_relaxed("{\"a\": 1, // c\n \"b\": [2,],}").unwrap()));
}
"##;

const WANT: &str = r##"{"z":1,"a":{"id":9007199254740993,"price":1.50,"e":1e3},"list":[true,null,"x\ny"],"z":2,"/k~":7}
{|  "z": 1,|  "a": {|    "id": 9007199254740993,|    "price": 1.50,|    "e": 1e3|  },|  "list": [|    true,|    null,|    "x\ny"|  ],|  "z": 2,|  "/k~": 7|}
keys z,a,list,z,/k~
z 2
id 9007199254740993
price 1.50 int=None
e int None
list1 null esc x\ny
ptr esc 7
missing None
json_of {"/k~":7,"a":{"e":1000,"id":9007199254740992,"price":1.5},"list":[true,null,"x\ny"],"z":2}
doc_of {"a":[1.5,true],"b":2}
built {"d":{"name":"ada","id":12345678901234567890}}
bad +1 doc=invalid number: +1 json=invalid number: +1
bad 01 doc=invalid number: 01 json=invalid number: 01
bad 1. doc=invalid number: 1. json=invalid number: 1.
bad .5 doc=invalid number: .5 json=invalid number: .5
bad - doc=invalid number: - json=invalid number: -
bad 1e doc=invalid number: 1e json=invalid number: 1e
bad [1,] doc=expected a number json=expected a number
bad {"a" 1} doc=expected ':' json=expected ':'
relaxed {"a":1,"b":[2]}
"##;

#[test]
fn json_doc_keeps_member_order_and_number_text() {
    let dir = std::env::temp_dir().join(format!("ray_json_doc_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.ray"), PROG).unwrap();
    for flags in [&[][..], &["--interp"][..]] {
        let out = Command::new(env!("CARGO_BIN_EXE_ray")).arg("run").args(flags).arg("a.ray").current_dir(&dir).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), WANT, "{flags:?}: {}", String::from_utf8_lossy(&out.stderr));
    }
}
