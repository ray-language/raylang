//! rayreq #260: el paquete `yaml` lee YAML 1.2 al `json.Doc` ordenado y lo escribe de vuelta. Las
//! pruebas de raylang son las del lector de rayreq (de donde nace el paquete) más la ida y vuelta
//! de `render`; se corren con `ray test` sobre un proyecto que depende del paquete por ruta.

use std::process::Command;

const TESTS: &str = r##"// YAML as API descriptions write it, read into the ordered tree; compared through its JSON.

import std/json;
import yaml/yaml;

fn as_json(text: string) -> string {
    match (yaml.parse(text)) {
        Result.Ok(n) => json.render_doc(n),
        Result.Err(e) => panic(e),
    }
}

fn error_of(text: string) -> string {
    match (yaml.parse(text)) {
        Result.Ok(n) => panic("accepted: ${json.render_doc(n)}"),
        Result.Err(e) => e,
    }
}

@test
fn mappings_sequences_and_comments() {
    let text = `# An API
openapi: 3.0.3   # the version
info:
  title: Pet store
  version: '1.0'
paths:
  /pets/{id}:
    get:
      tags: [pets, "store"]
      parameters:
        - name: id
          in: path
          required: true
        -   name: verbose
            in: query
      responses:
        '200':
          description: ok
        404: {description: missing}
list:
- a
- b: 1
  c: 2
-
  - nested
empty:
after: x
`;
    assert_eq(
        as_json(text),
        `{"openapi":"3.0.3","info":{"title":"Pet store","version":"1.0"},"paths":{"/pets/{id}":{"get":{"tags":["pets","store"],"parameters":[{"name":"id","in":"path","required":true},{"name":"verbose","in":"query"}],"responses":{"200":{"description":"ok"},"404":{"description":"missing"}}}}},"list":["a",{"b":1,"c":2},["nested"]],"empty":null,"after":"x"}`
    );
}

@test
fn scalars_follow_the_core_schema() {
    assert_eq(
        as_json(
            `[~, null, true, False, 12, -3, +4, 0x1F, 0o17, 1.5, .5, 2., 1e3, 007, 3.0.1, "12", yes, .inf]`
        ),
        `[null,null,true,false,12,-3,4,31,15,1.5,0.5,2.0,1e3,7,"3.0.1","12","yes",".inf"]`
    );
    assert_eq(as_json("a: !!str 12\nb: !custom x"), `{"a":"12","b":"x"}`);
    assert_eq(
        as_json("k: a # comment\nu: http://x/#frag\nh: a#b"),
        `{"k":"a","u":"http://x/#frag","h":"a#b"}`
    );
}

@test
fn quoted_scalars() {
    assert_eq(
        as_json(
            `a: 'it''s'
b: "tab\\there \\u00e9 \\x41 \\"q\\" \\\\"
c: "one
  two

  three"
d: 'x: y # not a comment'
"quoted key": 1
'single key': 2`
        ),
        `{"a":"it's","b":"tab\\there é A \\"q\\" \\\\","c":"one two\\nthree","d":"x: y # not a comment","quoted key":1,"single key":2}`
    );
    assert_eq(as_json(`a: "join\\
   ed"`), `{"a":"joined"}`);
}

@test
fn multi_line_plain_scalars() {
    assert_eq(
        as_json("description: A long\n  line that\n\n  goes on\nnext: 1"),
        `{"description":"A long line that\\ngoes on","next":1}`
    );
}

@test
fn block_scalars() {
    let text = `lit: |
  line 1
    indented

  line 3
fold: >
  a
  b

  c
    keep
  d
strip: |-
  x

keep: |+
  y

clip: >
  z


explicit: |2
    two more
end: 1
`;
    assert_eq(
        as_json(text),
        `{"lit":"line 1\\n  indented\\n\\nline 3\\n","fold":"a b\\nc\\n  keep\\nd\\n","strip":"x","keep":"y\\n\\n","clip":"z\\n","explicit":"  two more\\n","end":1}`
    );
    assert_eq(as_json("- |\n  in a list\n- after"), `["in a list\\n","after"]`);
    assert_eq(as_json("e: |\nn: 1"), `{"e":"","n":1}`);
}

@test
fn flow_collections_across_lines() {
    assert_eq(as_json(`x: {a: 1, "b": [1, 2,
   3], c: , d: 'q'}
y: [ ]
z: {}
w: [a: 1, b]`), `{"x":{"a":1,"b":[1,2,3],"c":null,"d":"q"},"y":[],"z":{},"w":[{"a":1},"b"]}`);
}

@test
fn anchors_aliases_and_merges() {
    let text = `base: &base
  type: object
  x: 1
copy: *base
merged:
  <<: *base
  x: 2
ref: &v value
again: *v
`;
    assert_eq(
        as_json(text),
        `{"base":{"type":"object","x":1},"copy":{"type":"object","x":1},"merged":{"type":"object","x":2},"ref":"value","again":"value"}`
    );
}

@test
fn documents_and_a_bom() {
    assert_eq(
        as_json(char_from_code(0xFEFF).unwrap().to_string() + "%YAML 1.2\n---\na: 1\n...\n"),
        `{"a":1}`
    );
    assert_eq(as_json("--- \na: 1\n---\nb: 2"), `{"a":1}`);
    assert_eq(as_json("a: 1\r\nb: 2\r\n"), `{"a":1,"b":2}`);
    assert_eq(as_json(""), "null");
    assert_eq(as_json("just text"), `"just text"`);
}

@test
fn json_is_yaml_too() {
    assert_eq(as_json(`{"a": [1, {"b": null}], "c": "d"}`), `{"a":[1,{"b":null}],"c":"d"}`);
}

@test
fn errors_say_the_line() {
    assert(error_of("a: 1\n  b: 2").contains("line 2"));
    assert(error_of("a:\n  - x\n  y: 1").contains("line 3"));
    assert(error_of("a: *nope").contains("unknown alias"));
    assert(error_of("a: [1, 2").contains("unterminated"));
    assert(error_of("a: 'open").contains("unterminated"));
    assert(error_of("? complex\n: x").contains("complex keys"));
    assert(error_of("a:\n\tb: 1").contains("tab"));
    assert(error_of("a: \"x\" y").contains("after a quoted scalar"));
}

fn roundtrip(text: string) {
    let d = yaml.parse(text).unwrap();
    let back = yaml.parse(yaml.render(d)).unwrap();
    assert_eq(json.render_doc(back), json.render_doc(d));
}

@test
fn render_reads_back_the_same_document() {
    roundtrip(`openapi: 3.0.3
info:
  title: Pet store
  version: '1.0'
  x: "a: b # c"
paths:
  /pets/{id}:
    get:
      tags: [pets, "store"]
      parameters:
        - name: id
          in: path
          required: true
          schema: {type: integer, format: int64}
        - [1, 2.50, null, "", '  sp  ']
      empty: {}
      none: []
      multi: |
        line one
        line two
`);
    assert_eq(yaml.render(json.parse_doc("{\"b\":1,\"a\":[true,\"x y\",{\"k\":\"-v\"}]}").unwrap()), "b: 1\na:\n  - true\n  - x y\n  - k: \"-v\"\n");
}
"##;

#[test]
fn the_yaml_package_reads_and_writes_yaml() {
    let pkg = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("packages/yaml");
    let app = std::env::temp_dir().join(format!("ray_yaml_pkg_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&app);
    std::fs::create_dir_all(app.join("src")).unwrap();
    std::fs::create_dir_all(app.join("tests")).unwrap();
    std::fs::write(
        app.join("ray.toml"),
        format!("[package]\nname = \"yt\"\nversion = \"0.1.0\"\n\n[dependencies]\nyaml = \"path:{}\"\n", pkg.display().to_string().replace('\\', "/")),
    )
    .unwrap();
    std::fs::write(app.join("src/main.ray"), "fn main() {}\n").unwrap();
    std::fs::write(app.join("tests/yaml_test.ray"), TESTS).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_ray")).arg("test").current_dir(&app).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("11 test(s), all passed"), "{stdout}");
}
