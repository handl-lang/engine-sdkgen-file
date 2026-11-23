# The `hnl-file` emitter — `.hnl` port/adapter files

`handl-plugin-codegen-emitter-file` is the reference `Emitter`: it renders
`SdkgenModule` IR into `Change`s — a Handl port/adapter pair — with no
fs writes (PL-004). `CodegenRegistry::run` reconciles the set through
`Engine::apply` over an `FsStore` rooted at `req.output`. It serves the
`c` and `webidl` targets — they compose it *by name* through the
`CodegenRegistry`, never by crate dep. Framework context:
[`host/crates/plugin-codegen`](https://github.com/handl-lang/host/tree/main/crates/plugin-codegen).

## Construction

`FileEmitter::new(Arc<Language>)` — the canonical `Language` is injected
via `CodegenRequest.lang` (the ADR-0128 "0.8 seam"): emitted source runs
through `handl_plugin_ext_fmt::format_source_with_language` so output
satisfies `hnl fmt --check` out of the box. On a formatter error the
raw text is still written — an unformatted artifact beats none.

## Output layout

```text
<output>/<module>.hnl                                — port
<output>/<lib>-<backend_id>-<slug>/<module>_c.hnl   — adapter
```

Adapters nest *inside* the store root (PL-004: `ArtifactRef` cannot
escape `req.output`) — the pre-kernel layout wrote them to a sibling
`<output>/../<adapter_pkg>/` dir.

`<lib>` = basename of `--output`; `<slug>` = `target_slug(
module.target)` (`x86_64-unknown-linux-gnu` → `linux-x86_64`; `browser`,
`host` pass through).

The adapter's `_c` suffix keeps the two basenames distinct in the
module table: the adapter file's own `from "../<module>" import`
line resolves the *port* spec (`<output>/<module>.hnl`) instead of
basename-colliding back onto itself (the `uchar` self-import class).

When `module.per_record_files` is set (the `webidl` backend sets it —
its records never name each other), the layout splits: one
`<Record>.hnl` port / `<Record>_c.hnl` adapter pair per record, plus
`types.hnl`/`types_c.hnl` for typedefs, enum constants, and functions
not owned by a record (ownership is `c_decl`'s prefix before `.` —
`Request.arrayBuffer` belongs to `Request`). Stale single-file
`<module>.hnl` (legacy) and `<module>_c.hnl` adapter artifacts are
removed on regen. Small files stay under typechecker node budgets.

## Port emission (`build_port`)

Order: header comments (`Generated port`, `<client> target:`,
`--- source:`), client + `result` imports, `module.imports` (backend-
declared — the emitter holds no backend vocabulary), then:

- **records** → `export Name := { field: Port, … }` with per-field docs
- **typedefs** → `export Name := <port>`
- **constants** → `export NAME: Int` (declaration only)
- **functions** → `export <Port>: (args) ~[1]~> Result<T, E>` where `E`
  is `Errno` (`ffi` client) or `JsError` (`js` client). Functions with
  independent out channels also declare `{Port}Result` —
  `{ ret: Int, buf?, outs?, vals? }`; functions whose signature
  references `{Port}Msg` (inline `in_struct:`/`io_struct:` overrides)
  get that record declared from the descriptor's field list
  (`name:kind@off[:size]`, brace-depth-aware split, `rec:` nesting).
- Doc layering output lands here: `--- doc` lines on exports and fields.

## Adapter emission (`build_adapter`)

Per client (`module.client_package`):

- **`ffi`** — `ffi.call(lib, sym, [args], [descs], ret, false)` via
  *qualified* call (`ffi.call`, `ffi.errno`), not `from "ffi" import`
  (the package's `call` export is a type signature). `lib` is
  `module.library`, defaulting to `<lib>.so.6` when empty. Enum
  constants emit `export NAME: Int := <value>` — target-specific data
  bound here, not in the port.
- **`js`** — `from "js" import { … }` listing only the ops the
  module's functions dispatch through (`call`, `invoke`, `construct`,
  `on` — plus `error` always). Dispatch is classified per function
  (`js_dispatch`) from backend-encoded markers, never from backend
  vocabulary:
  - `c_decl` ending in `.constructor` → `construct("", "<Interface>",
    …)` (`Reflect.construct` on the global).
  - leading `recv:<desc>` arg descriptor → `invoke(<recv>, "<member>",
    …)` — the receiver is peeled out of the marshalled arg/descriptor
    lists.
  - `handler:<event>` arg descriptor (behind `recv:`) →
    `on(<recv>, "<event>", <handler>)` — listener registration; its
    failure convention differs (`js.on` returns the listener or null
    and does not clear `error()` on success), so the adapter checks
    `r == null && e != null`.
  - anything else → `call("<owner>", "<member>", …)` where `<owner>`
    is `c_decl`'s prefix (namespace object, interface global for
    `static` ops) or `module.library` for owner-less functions.
  `method` is `c_decl`'s suffix after `.`; descriptors pass through
  `js_descriptor` (`void`→`js_void`, `bool`→`js_boolean`,
  numerics→`js_number`, `promise*`→`js_promise`, `js_*` spellings
  untouched; `recv:`/`handler:` markers never reach it — `recv:` args
  are skipped positionally). Result: `ok(r)` unless `error()` is
  non-null (`on` uses the null-result convention above).

Return unwrapping for `ffi` (skipped for `js`):

| Shape | Emitted |
| --- | --- |
| `{Port}Result` declared | `if r.ret < 0 { err } else { ok(r) }` (whole record) — or `ok(r)` when `errno_guard` off |
| lone `out_buf:` | `ret` doubles as byte count → `ok(r.buf)` (guarded on `r.ret`) |
| `ret_descriptor == "void"` | `ok(null)` |
| numeric return | `r` itself, `r < 0` errno guard |
| other | `ok(r)` |

`errno_guard: false` drops the `ret < 0`/`r < 0` check on every arm —
libraries whose failures arrive out-of-band (`UErrorCode*`, Win32
`GetLastError`) keep valid negative returns.

Adapter bodies are plain `~>` arrows (`export f: Port := (args) ~> do
{…}`): the grade lives on the port annotation; a `~[N]~>` in
expression position would evaluate eagerly at module load. Argument
names colliding with canonical Handl keywords get a `_` suffix
(`data` → `data_`) — the `HANDL_KEYWORDS` list mirrors
`lexer_rules.yaml`.

## Descriptor → port-arg contract

Emitters consume `arg_descriptors` positionally (one per foreign arg);
`port_args` lists only the consuming ones. The no-consume set:
`out_buf:*`, `out_ptr:*`, `io_buf:*`, `out_struct:*`, `io_struct:*`,
`void` (`io_scalar:*` consumes — the caller seeds the cell). See
[host/crates/plugin-codegen](https://github.com/handl-lang/host/tree/main/crates/plugin-codegen)
for the cross-backend vocabulary table.
