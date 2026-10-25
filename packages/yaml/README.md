# yaml

YAML 1.2 for raylang: read a document into `std/json`'s ordered `Doc` and write a `Doc` back
as YAML. Object members keep the order they were written in and numbers keep their text, so
an OpenAPI description, a CI workflow or a Kubernetes manifest reads the same way it was
written — and the same code handles it whether it came as JSON (`json.parse_doc`) or YAML.

```toml
[dependencies]
yaml = "^0.1"
```

```rust
import std/json;
import yaml/yaml;

fn main() -> int {
    let text = `openapi: 3.0.3
info:
  title: Pet store
paths:
  /pets/{id}:
    get:
      tags: [pets]
`;
    let doc = yaml.parse(text).unwrap();
    print(json.doc_text(json.doc_pointer(doc, "/info/title").unwrap()).unwrap()); // Pet store
    for m in json.doc_members(json.doc_get(doc, "paths").unwrap()) {
        print(m.key); // /pets/{id}
    }
    print(json.render_doc(doc)); // the same document as JSON, members in order
    print(yaml.render(doc));     // and back to YAML
    0
}
```

## API

- `parse(text) -> Result<json.Doc, string>` — the first document of `text`. Block mappings and
  sequences (also a sequence at its key's indentation), flow `[…]`/`{…}`, plain, single- and
  double-quoted and multi-line scalars, `|`/`>` block scalars with their indicators, comments,
  anchors, aliases and the `<<` merge key. Errors name the line.
- `render(doc) -> string` — block style, two spaces per level, members in order; a string stays
  plain when that cannot change its meaning and goes double-quoted otherwise. `parse(render(d))`
  gives `d` back.
- `plain_scalar(s) -> json.Doc` — what an unquoted scalar means under the YAML 1.2 core schema:
  `null`/`~`/empty, `true`/`false`, integers and floats (`0x1F` → `31`), a string otherwise.

Read the result with `std/json`'s `Doc` helpers: `doc_get`, `doc_at`, `doc_members`,
`doc_items`, `doc_text`, `doc_int` (exact 64-bit), `doc_float`, `doc_bool`, `doc_pointer`
(JSON Pointer), `render_doc`/`render_doc_pretty`, and `json_of` to get a `Json`.

## Limits

- Only the first document of a multi-document stream is read.
- Complex keys (`? `) are not supported; tags are skipped (`!!str` keeps a scalar as text).
- A repeated key keeps the last value, in the place of the first.

The reader comes from rayreq (ray-apps), where it was checked against libyaml on the OpenAPI
descriptions of GitHub (10 MB) and Stripe (6.6 MB). Requires raylang 1.27.80 or later.
