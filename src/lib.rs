//! File emitter for the Handl codegen pipeline (ADR-0107, PL-004).

use handl_language::Language;
use handl_plugin_codegen::{
    Change, CodegenEmitterDef, CodegenError, CodegenRegistry, EmitContext, Emitter, SdkgenModule,
    erase_emitter,
};
use std::sync::Arc;

/// Emitter that renders port and adapter `.hnl` files as `Change`s.
///
/// The [`Language`] used to canonicalize emitted source is injected at
/// construction (ADR-0128 §"The 0.8 seam"): the emitter lives in platform,
/// so it must not link `handl-kit-data-prelude` itself — the host
/// (`hnl codegen`) assembles the canonical language and hands it in.
pub struct FileEmitter {
    lang: Arc<Language>,
}

impl FileEmitter {
    pub fn new(lang: Arc<Language>) -> Self {
        Self { lang }
    }
}

/// Register the `hnl-file` emitter on `reg` (ADR-0128 §0.1, PL-004):
/// the emitter id the `c`/`webidl` target records reference. The
/// request's `lang` is cloned per emit — `FileEmitter` owns it behind
/// an `Arc`.
pub fn register(reg: &mut CodegenRegistry) {
    reg.register_emitter(CodegenEmitterDef {
        name: "hnl-file",
        make: |req| Ok(erase_emitter(FileEmitter::new(Arc::new(req.lang.clone())))),
    });
}

impl Emitter<SdkgenModule> for FileEmitter {
    /// Render the desired `.hnl` artifact set — no fs writes (PL-004).
    /// `req.output_dir` is the port package dir (e.g.
    /// `handl-packages/generated/libc`); port files land at its root
    /// (`<module>.hnl`) and adapters under a nested `<adapter_pkg>/`
    /// subdir. The pre-kernel layout wrote adapters to a *sibling*
    /// `<port_pkg>-<backend>-<target>/` dir — `ArtifactRef` cannot
    /// escape the store root, so T-076 must update consumer
    /// `.handl.toml` dep paths (or revisit the `--output` contract)
    /// when the sibling layout is restored.
    fn materialize(
        &self,
        module: &SdkgenModule,
        req: &EmitContext,
    ) -> Result<Vec<Change>, CodegenError> {
        let output = req.output_dir.as_deref().ok_or_else(|| {
            CodegenError::Emit("hnl-file emitter requires EmitContext::output_dir".to_string())
        })?;

        let lib = output
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("lib")
            .to_string();
        let module_name = &module.module;
        let target_slug = target_slug(&module.target);
        let adapter_pkg = format!("{lib}-{}-{}", module.backend_id, target_slug);

        if module.per_record_files {
            return Ok(emit_per_record(&self.lang, module, &lib, &adapter_pkg));
        }

        let port = canonicalize(&self.lang, build_port(module, &lib, module_name));
        let adapter = canonicalize(
            &self.lang,
            build_adapter(module, &lib, module_name, &adapter_pkg),
        );

        // The adapter file carries a `_c` suffix (`uchar_c.hnl` next to
        // the port's `uchar.hnl`): both basenames resolve through the
        // module table, and a same-named adapter would self-import via
        // its own `from "../<name>"` line (the `uchar` collision —
        // the port keeps the bare name, the adapter the impl suffix).
        Ok(vec![
            Change::text(format!("{module_name}.hnl"), port),
            Change::text(format!("{adapter_pkg}/{module_name}_c.hnl"), adapter),
        ])
    }
}

/// Per-record layout: one `<Record>.hnl` port / `<Record>_c.hnl` adapter
/// pair per record plus `types.hnl`/`types_c.hnl` for typedefs, enum
/// constants, and functions not owned by a record (namespace members,
/// callbacks). The single-file layout's `<module>.hnl` (legacy) and
/// `<module>_c.hnl` adapter artifacts are declared as `Change::Delete`
/// so a regenerated spec doesn't leave a stale, oversized file behind.
fn emit_per_record(
    lang: &Language,
    module: &SdkgenModule,
    lib: &str,
    adapter_pkg: &str,
) -> Vec<Change> {
    let module_name = module.module.as_str();
    let mut changes = vec![
        Change::delete(format!("{module_name}.hnl")),
        Change::delete(format!("{adapter_pkg}/{module_name}_c.hnl")),
        // Legacy pre-`_c` adapter artifact — removed too so an older
        // emission doesn't linger under the colliding bare name.
        Change::delete(format!("{adapter_pkg}/{module_name}.hnl")),
    ];

    let record_names: std::collections::HashSet<&str> =
        module.records.iter().map(|r| r.name.as_str()).collect();

    // A function belongs to the interface its `c_decl` names
    // (`Request.arrayBuffer` → `Request`); anything else lands in
    // `types.hnl`.
    let mut owned: std::collections::HashMap<&str, Vec<handl_plugin_codegen::Function>> =
        std::collections::HashMap::new();
    let mut loose: Vec<handl_plugin_codegen::Function> = Vec::new();
    for f in &module.functions {
        let owner = f.c_decl.split('.').next().unwrap_or("");
        if record_names.contains(owner) {
            owned.entry(owner).or_default().push(f.clone());
        } else {
            loose.push(f.clone());
        }
    }

    for rec in &module.records {
        let mut sub = module.clone();
        sub.records = vec![rec.clone()];
        sub.functions = owned.remove(rec.name.as_str()).unwrap_or_default();
        sub.typedefs = Vec::new();
        sub.enum_constants = Vec::new();
        changes.extend(pair_changes(lang, &sub, lib, &rec.name, adapter_pkg));
    }

    let mut types = module.clone();
    types.records = Vec::new();
    types.functions = loose;
    if !types.typedefs.is_empty() || !types.enum_constants.is_empty() || !types.functions.is_empty()
    {
        changes.extend(pair_changes(lang, &types, lib, "types", adapter_pkg));
    }
    changes
}

/// The port + adapter `Change` pair for one emitted module slice.
fn pair_changes(
    lang: &Language,
    sub: &SdkgenModule,
    lib: &str,
    name: &str,
    adapter_pkg: &str,
) -> [Change; 2] {
    let port = canonicalize(lang, build_port(sub, lib, name));
    let adapter = canonicalize(lang, build_adapter(sub, lib, name, adapter_pkg));
    [
        Change::text(format!("{name}.hnl"), port),
        Change::text(format!("{adapter_pkg}/{name}_c.hnl"), adapter),
    ]
}

/// Run emitted source through the canonical formatter so generated files
/// satisfy `hnl fmt --check` out of the box (e.g. `Result<(), E>` renders as
/// `Result < (), E >`). On a format error the raw text is still written — an
/// unformatted artifact is more debuggable than none.
fn canonicalize(lang: &Language, src: String) -> String {
    match handl_plugin_ext_fmt::format_source_with_language(lang, &src) {
        Ok(formatted) => formatted,
        Err(e) => {
            eprintln!("codegen: emitted source failed canonical fmt ({e}); emitting unformatted");
            src
        }
    }
}

fn target_slug(target: &str) -> String {
    let parts: Vec<&str> = target.split('-').collect();
    // Non-triple targets (e.g. `browser`, `node`, `deno`) have no arch/os
    // to extract — use the name verbatim instead of `unknown-<name>`.
    if parts.len() == 1 {
        return target.to_string();
    }
    let arch = parts.first().copied().unwrap_or("unknown");
    let os = parts
        .get(2)
        .copied()
        .or(parts.get(1).copied())
        .unwrap_or("unknown");
    format!("{os}-{arch}")
}

struct ClientInfo {
    client_pkg: &'static str,
    error_type: &'static str,
}

fn client_info(client_package: &str) -> ClientInfo {
    if client_package == "js" {
        ClientInfo {
            client_pkg: "js",
            error_type: "JsError",
        }
    } else {
        ClientInfo {
            client_pkg: "ffi",
            error_type: "Errno",
        }
    }
}

fn build_port(module: &SdkgenModule, lib: &str, name: &str) -> String {
    let client = client_info(&module.client_package);
    let mut out = String::new();
    out.push_str(&format!("--- Generated port for {lib}.{name}\n"));
    out.push_str(&format!(
        "--- {} target: {}\n",
        client.client_pkg, module.target
    ));
    if let Some(source) = &module.source {
        out.push_str(&format!("--- source: {source}\n"));
    }
    out.push('\n');
    out.push_str(&format!(
        "from \"{}\" import {{ {} }}\n",
        client.client_pkg, client.error_type
    ));
    out.push_str("from \"result\" import { Result }\n");
    // Backend-declared imports (e.g. the C backend's `types/c` aliases) —
    // rendered as data so the emitter stays backend-agnostic.
    for imp in &module.imports {
        out.push_str(&format!(
            "from \"{}\" import {{ {} }}\n",
            imp.package,
            imp.names.join(", ")
        ));
    }
    out.push('\n');

    for rec in &module.records {
        if let Some(d) = &rec.doc {
            for line in d.lines() {
                out.push_str(&format!("--- {}\n", line.trim()));
            }
        }
        out.push_str(&format!("export {} := {{\n", rec.name));
        for (i, f) in rec.fields.iter().enumerate() {
            if let Some(d) = &f.doc {
                for line in d.lines() {
                    out.push_str(&format!("  --- {}\n", line.trim()));
                }
            }
            let comma = if i == rec.fields.len() - 1 { "" } else { "," };
            out.push_str(&format!("  {}: {}{}\n", f.name, f.port, comma));
        }
        out.push_str("}\n\n");
    }

    for td in &module.typedefs {
        if let Some(d) = &td.doc {
            for line in d.lines() {
                out.push_str(&format!("--- {}\n", line.trim()));
            }
        }
        out.push_str(&format!("export {} := {}\n\n", td.name, td.port));
    }

    for c in &module.enum_constants {
        if let Some(d) = &c.doc {
            for line in d.lines() {
                out.push_str(&format!("--- {}\n", line.trim()));
            }
        }
        out.push_str(&format!("export {}: Int\n", c.name));
    }
    if !module.enum_constants.is_empty() {
        out.push('\n');
    }

    for f in &module.functions {
        let arg_list = f
            .port_args
            .iter()
            .map(|a| a.port.clone())
            .collect::<Vec<_>>()
            .join(", ");
        if let Some(d) = &f.doc {
            for line in d.lines() {
                out.push_str(&format!("--- {}\n", line.trim()));
            }
        }
        let result_fields = result_record_fields(f);
        if let Some(fields) = &result_fields {
            // Struct/scalar/multi-out functions return the full marshal
            // record — declared here as `{Port}Result`.
            out.push_str(&format!("export {}Result := {{\n", f.port_name));
            for (i, (fname, port)) in fields.iter().enumerate() {
                let comma = if i == fields.len() - 1 { "" } else { "," };
                out.push_str(&format!("  {fname}: {port}{comma}\n"));
            }
            out.push_str("}\n\n");
        }
        // in_struct/io_struct functions whose port signature references
        // `<Port>Msg` (overrides, or the `buf` of a `{Port}Result`)
        // declare that record shape here. Auto-mapped `const struct T*`
        // args are typed as the record itself (e.g. timespec), so no
        // per-function shape is emitted.
        let msg_name = format!("{}Msg", f.port_name);
        let uses_msg = f.port_args.iter().any(|a| a.port == msg_name)
            || f.return_port == msg_name
            || result_fields
                .as_ref()
                .is_some_and(|fs| fs.iter().any(|(n, t)| n == "buf" && *t == msg_name));
        if uses_msg && let Some(fields) = msg_record_fields(f) {
            out.push_str(&format!("export {msg_name} := {{\n"));
            for (i, (fname, port)) in fields.iter().enumerate() {
                let comma = if i == fields.len() - 1 { "" } else { "," };
                out.push_str(&format!("  {fname}: {port}{comma}\n"));
            }
            out.push_str("}\n\n");
        }
        out.push_str(&format!("--- {}: {}\n", client.client_pkg, f.c_decl));
        out.push_str(&format!(
            "export {}: ({}) ~[1]~> Result<{}, {}>\n\n",
            f.port_name, arg_list, f.return_port, client.error_type
        ));
    }

    out
}

fn is_byte_slot(d: &str) -> bool {
    d.starts_with("out_buf:") || d.starts_with("out_ptr:") || d.starts_with("io_buf:")
}

fn is_struct_slot(d: &str) -> bool {
    d.starts_with("out_struct:") || d.starts_with("io_struct:")
}

/// Field list for a function's `{Port}Result` record — `None` when the
/// adapter unwraps the return to a plain value (a lone `out_buf`, where
/// `ret` doubles as the byte count). Every other out-shape — struct outs,
/// scalar value-results, `out_ptr`/`io_buf`, multiple byte slots — keeps
/// `ret` visible since it carries independent meaning.
fn result_record_fields(f: &handl_plugin_codegen::Function) -> Option<Vec<(String, String)>> {
    let byte_slots = f.arg_descriptors.iter().filter(|d| is_byte_slot(d)).count();
    let struct_out = f.out_struct.is_some() || f.arg_descriptors.iter().any(|d| is_struct_slot(d));
    let scalar_outs = f
        .arg_descriptors
        .iter()
        .any(|d| d.starts_with("io_scalar:"));
    let needs = struct_out
        || scalar_outs
        || byte_slots > 1
        || f.arg_descriptors
            .iter()
            .any(|d| d.starts_with("out_ptr:") || d.starts_with("io_buf:"));
    if !needs {
        return None;
    }
    let mut fields = vec![("ret".to_string(), "Int".to_string())];
    if struct_out {
        // Auto-mapped out_structs name the record (`timespec`); override
        // io_structs get a per-function `{Port}Msg` shape.
        let ty = f
            .out_struct
            .clone()
            .unwrap_or_else(|| format!("{}Msg", f.port_name));
        fields.push(("buf".to_string(), ty));
    } else if byte_slots > 0 {
        fields.push(("buf".to_string(), "Bytes".to_string()));
    }
    if byte_slots > 0 {
        fields.push(("outs".to_string(), "[Bytes]".to_string()));
    }
    if scalar_outs {
        fields.push(("vals".to_string(), "[Int]".to_string()));
    }
    Some(fields)
}

/// Parse the field list of an `in_struct:{...}`/`io_struct:{...}`
/// descriptor into `(name, port-type)` pairs for the port's record decl.
/// Field specs carry layout suffixes (`name:kind@off[:size]`) and may
/// nest (`rec:{...}`), so splitting must respect brace depth.
fn msg_record_fields(f: &handl_plugin_codegen::Function) -> Option<Vec<(String, String)>> {
    let desc = f
        .arg_descriptors
        .iter()
        .find(|d| d.starts_with("in_struct:") || d.starts_with("io_struct:"))?;
    let inner = desc
        .split_once(':')
        .map(|(_, r)| r)?
        .strip_prefix('{')?
        .strip_suffix('}')?;
    let fields = split_top_fields(inner)
        .into_iter()
        .filter_map(|p| {
            let (name, spec) = p.split_once(':')?;
            Some((name.trim().to_string(), field_port(spec_head(spec))))
        })
        .collect();
    Some(fields)
}

/// Split a descriptor field list on commas at brace depth zero —
/// `rec:{a:i32@0,b:i32@8}` contains commas that must not split.
fn split_top_fields(inner: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    for ch in inner.chars() {
        match ch {
            '{' => {
                depth += 1;
                cur.push(ch);
            }
            '}' => {
                depth -= 1;
                cur.push(ch);
            }
            ',' if depth == 0 => parts.push(std::mem::take(&mut cur)),
            _ => cur.push(ch),
        }
    }
    if !cur.trim().is_empty() {
        parts.push(cur);
    }
    parts
}

/// The field kind before its `@offset[:size]` layout suffix — the `@`
/// may hide inside `rec:{...}`, so scan at depth zero only.
fn spec_head(spec: &str) -> &str {
    let mut depth = 0i32;
    for (i, ch) in spec.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => depth -= 1,
            '@' if depth == 0 => return &spec[..i],
            _ => {}
        }
    }
    spec.trim()
}

/// Port type for a struct field kind.
fn field_port(spec: &str) -> String {
    let spec = spec.trim();
    if spec == "bytes" || spec.starts_with("buf:") {
        "Bytes".into()
    } else if spec.starts_with("arr:") {
        if spec.ends_with(":u8") {
            "Bytes".into()
        } else {
            "[Int]".into()
        }
    } else if spec == "iov" || spec.starts_with("iovbuf:") {
        "[Bytes]".into()
    } else if spec == "ptr" {
        "Int".into()
    } else if spec.starts_with("rec:") {
        "Obj".into()
    } else if spec.starts_with('u') || spec == "isize" || spec == "usize" {
        "Nat".into()
    } else {
        "Int".into()
    }
}

/// How a `js`-client function dispatches at runtime. The webidl
/// backend encodes the dispatch kind in the function's descriptors and
/// `c_decl`:
///
/// - `Interface.constructor` → [`JsDispatch::Construct`] (`js.construct`
///   — `Reflect.construct` on the global constructor named by the
///   interface).
/// - a leading `recv:<desc>` arg descriptor → [`JsDispatch::Invoke`]
///   (`js.invoke` — the arg is the receiver, peeled out of the marshalled
///   arg list).
/// - a `handler:<event>` arg descriptor (always behind `recv:`) →
///   [`JsDispatch::On`] (`js.on` — registers the arrow as an event
///   listener; no descriptor list applies).
/// - anything else → [`JsDispatch::Call`] (`js.call` on the
///   `Interface.member` owner — the namespace object for namespace ops,
///   the interface global for `static` ops).
enum JsDispatch {
    Call {
        obj: String,
        method: String,
    },
    Invoke {
        recv: String,
        method: String,
    },
    Construct {
        method: String,
    },
    On {
        recv: String,
        event: String,
        arrow: String,
    },
}

/// Classify `f`'s JS dispatch. `library` is the module fallback object
/// for `c_decl`s that name no owner (callbacks and other loose
/// functions).
fn js_dispatch(f: &handl_plugin_codegen::Function, library: &str) -> JsDispatch {
    let (owner, member) = f
        .c_decl
        .rsplit_once('.')
        .map(|(o, m)| (Some(o), m))
        .unwrap_or((None, f.name.as_str()));
    let recv_marked = f
        .arg_descriptors
        .first()
        .is_some_and(|d| d.starts_with("recv:"));

    if let Some(pos) = f
        .arg_descriptors
        .iter()
        .position(|d| d.starts_with("handler:"))
        && recv_marked
        && pos > 0
        && pos < f.port_args.len()
    {
        return JsDispatch::On {
            recv: safe_ident(&f.port_args[0].name),
            event: f.arg_descriptors[pos]["handler:".len()..].to_string(),
            arrow: safe_ident(&f.port_args[pos].name),
        };
    }

    if member == "constructor"
        && let Some(intf) = owner
    {
        return JsDispatch::Construct {
            method: intf.to_string(),
        };
    }

    if recv_marked && owner.is_some() {
        return JsDispatch::Invoke {
            recv: safe_ident(&f.port_args[0].name),
            method: member.to_string(),
        };
    }

    JsDispatch::Call {
        obj: owner.unwrap_or(library).to_string(),
        method: member.to_string(),
    }
}

/// The `js` ops a module's functions actually dispatch through — the
/// adapter's `from "js" import { … }` list, `error` always included.
fn js_ops_used(module: &SdkgenModule) -> Vec<&'static str> {
    let mut used = [false; 4]; // call, invoke, construct, on
    for f in &module.functions {
        match js_dispatch(f, &module.library) {
            JsDispatch::Call { .. } => used[0] = true,
            JsDispatch::Invoke { .. } => used[1] = true,
            JsDispatch::Construct { .. } => used[2] = true,
            JsDispatch::On { .. } => used[3] = true,
        }
    }
    const NAMES: [&str; 4] = ["call", "invoke", "construct", "on"];
    let mut ops: Vec<&'static str> = NAMES
        .iter()
        .zip(used)
        .filter_map(|(n, u)| u.then_some(*n))
        .collect();
    ops.push("error");
    ops
}

fn build_adapter(module: &SdkgenModule, lib: &str, name: &str, adapter_pkg: &str) -> String {
    let client = client_info(&module.client_package);
    let mut out = String::new();
    out.push_str(&format!(
        "--- Generated adapter for {adapter_pkg}/{name}_c\n"
    ));
    if let Some(source) = &module.source {
        out.push_str(&format!("--- source: {source}\n"));
    }
    out.push('\n');
    // `ffi.call`/`ffi.errno` resolve through the host module chain, not the
    // package table — `from "ffi" import { call }` would bind the *type
    // signature* `ffi.hnl` exports, which is not an arrow. The js client
    // keeps the named import (its package exports callable arrows); only
    // the ops the module's dispatch forms need are imported.
    if client.client_pkg == "js" {
        out.push_str(&format!(
            "from \"js\" import {{ {} }}\n",
            js_ops_used(module).join(", ")
        ));
    }
    out.push_str("from \"result\" import { ok, err }\n");

    let mut imports = Vec::new();
    for rec in &module.records {
        imports.push(rec.name.clone());
    }
    for f in &module.functions {
        imports.push(f.port_name.clone());
    }
    if !imports.is_empty() {
        // Adapters live one level below the port dir (`<adapter_pkg>/`
        // inside `req.output_dir`), so the port import is `../{name}`.
        out.push_str(&format!(
            "from \"../{name}\" import {{ {} }}\n",
            imports.join(", ")
        ));
    }
    out.push('\n');

    // Constants are pure values — no ffi.call involved. The port file
    // declares the type; the adapter binds the (target-specific) value.
    for c in &module.enum_constants {
        out.push_str(&format!("export {}: Int := {}\n", c.name, c.value));
    }
    if !module.enum_constants.is_empty() {
        out.push('\n');
    }

    for f in &module.functions {
        let arg_names = f
            .port_args
            .iter()
            .map(|a| safe_ident(&a.name))
            .collect::<Vec<_>>()
            .join(", ");

        let descriptors: Vec<String> = f
            .arg_descriptors
            .iter()
            .map(|d| {
                let mapped = if client.client_pkg == "js" {
                    js_descriptor(d)
                } else {
                    d.clone()
                };
                format!("\"{mapped}\"")
            })
            .collect();
        let descs = descriptors.join(", ");

        let ret_descriptor = if client.client_pkg == "js" {
            js_descriptor(&f.ret_descriptor)
        } else {
            f.ret_descriptor.clone()
        };

        // The grade lives on the port annotation (`f.port_name` carries
        // `~[1]~>`); the body must be a plain `~>` arrow. `~[N]~>` in
        // expression position builds a `dim_arrow` type record and
        // evaluates its children eagerly — params would be unbound and the
        // `call` would run at module load.
        out.push_str(&format!(
            "export {}: {} := ({}) ~> do {{\n",
            f.name, f.port_name, arg_names
        ));

        if client.client_pkg == "js" {
            // Descriptor/arg slices per dispatch form: `invoke` peels the
            // leading `self` receiver out of the marshalled lists; `on`
            // takes no descriptor list at all.
            let skip = match js_dispatch(f, &module.library) {
                JsDispatch::Invoke { .. } => 1,
                _ => 0,
            };
            let call_args: Vec<String> = f
                .port_args
                .iter()
                .skip(skip)
                .map(|a| safe_ident(&a.name))
                .collect();
            let call_descs: Vec<String> = f
                .arg_descriptors
                .iter()
                .skip(skip)
                .map(|d| format!("\"{}\"", js_descriptor(d)))
                .collect();
            let call_args = call_args.join(", ");
            let call_descs = call_descs.join(", ");
            match js_dispatch(f, &module.library) {
                JsDispatch::Call { obj, method } => out.push_str(&format!(
                    "  r := call(\"{obj}\", \"{method}\", [{call_args}], [{call_descs}], \"{ret_descriptor}\", false)\n"
                )),
                JsDispatch::Invoke { recv, method } => out.push_str(&format!(
                    "  r := invoke({recv}, \"{method}\", [{call_args}], [{call_descs}], \"{ret_descriptor}\", false)\n"
                )),
                JsDispatch::Construct { method } => out.push_str(&format!(
                    "  r := construct(\"\", \"{method}\", [{call_args}], [{call_descs}], \"{ret_descriptor}\", false)\n"
                )),
                JsDispatch::On { recv, event, arrow } => out.push_str(&format!(
                    "  r := on({recv}, \"{event}\", {arrow})\n"
                )),
            }
            if matches!(js_dispatch(f, &module.library), JsDispatch::On { .. }) {
                // `js.on` returns the listener or null on failure and does
                // not clear `error()` on success — only a null result with
                // a recorded exception is a failure.
                out.push_str("  e := error()\n");
                out.push_str("  if r == null && e != null {\n");
                out.push_str("    err(e)\n");
                out.push_str("  } else {\n");
                out.push_str("    ok(r)\n");
                out.push_str("  }\n");
            } else {
                out.push_str("  e := error()\n");
                out.push_str("  if e == null {\n");
                out.push_str("    ok(r)\n");
                out.push_str("  } else {\n");
                out.push_str("    err(e)\n");
                out.push_str("  }\n");
            }
        } else {
            out.push_str(&format!(
                "  r := ffi.call(\"{}\", \"{}\", [{arg_names}], [{descs}], \"{}\", false)\n",
                if module.library.is_empty() {
                    format!("{lib}.so.6")
                } else {
                    module.library.clone()
                },
                f.name,
                ret_descriptor
            ));
            if result_record_fields(f).is_some() {
                if f.errno_guard {
                    out.push_str("  if r.ret < 0 {\n");
                    out.push_str("    err(ffi.errno())\n");
                    out.push_str("  } else {\n");
                    out.push_str("    ok(r)\n");
                    out.push_str("  }\n");
                } else {
                    out.push_str("  ok(r)\n");
                }
            } else if f.arg_descriptors.iter().any(|d| d.starts_with("out_buf:")) {
                // A lone out_buf: `ret` doubles as the byte count — unwrap
                // to the buffer.
                if f.errno_guard {
                    out.push_str("  if r.ret < 0 {\n");
                    out.push_str("    err(ffi.errno())\n");
                    out.push_str("  } else {\n");
                    out.push_str("    ok(r.buf)\n");
                    out.push_str("  }\n");
                } else {
                    out.push_str("  ok(r.buf)\n");
                }
            } else if f.ret_descriptor == "void" {
                out.push_str("  ok(null)\n");
            } else if is_numeric_descriptor(&f.ret_descriptor) {
                if f.errno_guard {
                    out.push_str("  if r < 0 {\n");
                    out.push_str("    err(ffi.errno())\n");
                    out.push_str("  } else {\n");
                    out.push_str("    ok(r)\n");
                    out.push_str("  }\n");
                } else {
                    out.push_str("  ok(r)\n");
                }
            } else {
                out.push_str("  ok(r)\n");
            }
        }

        out.push_str("}\n\n");
    }

    out
}

fn js_descriptor(d: &str) -> String {
    match d {
        "js_string" | "js_number" | "js_object" | "js_boolean" | "js_promise" | "js_void" => {
            d.to_string()
        }
        "void" => "js_void".to_string(),
        "bool" => "js_boolean".to_string(),
        "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64" | "f32" | "f64" => {
            "js_number".to_string()
        }
        s if s.starts_with("promise:") || s == "promise" => "js_promise".to_string(),
        _ => d.to_string(),
    }
}

fn is_numeric_descriptor(d: &str) -> bool {
    d.starts_with('i') || d.starts_with('u') || d == "f64"
}

/// Canonical Handl keywords (mirrors `lex.keyword.*` in
/// `handl-kit-data-prelude/data/lexer_rules.yaml`). Spec argument names
/// that collide (`data`, `type`, `for`, …) cannot be emitted verbatim in
/// binder position — the adapter's `(params) ~> do` would not parse.
/// Suffix `_` keeps the name legible and consistent between the param
/// list and the call-args list.
const HANDL_KEYWORDS: &[&str] = &[
    "absurd", "app", "arr", "as", "axiom", "data", "do", "else", "export", "first", "for", "from",
    "grade", "grades", "handle", "if", "id2", "impl", "import", "in", "infer", "law", "left",
    "match", "op", "rec", "return", "rewrite", "right", "second", "sort", "then", "trait",
    "type", "where", "while", "with",
];

fn safe_ident(name: &str) -> String {
    if HANDL_KEYWORDS.contains(&name) {
        format!("{name}_")
    } else {
        name.to_string()
    }
}
