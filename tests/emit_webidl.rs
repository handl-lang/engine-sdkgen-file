use std::fs;
use std::path::PathBuf;

use handl_plugin_codegen::*;
use handl_plugin_codegen_emitter_file::FileEmitter;

/// The canonical language, assembled from the dev-dep data crate.
fn lang() -> std::sync::Arc<handl_language::Language> {
    std::sync::Arc::new(
        handl_kit_data_prelude::language_from_yaml().expect("canonical language assembles"),
    )
}

/// `materialize` the module into `out` through the kernel lifecycle
/// (`FsStore` + `Engine::apply`) — the same path
/// `CodegenRegistry::run` drives (PL-004).
fn emit(module: &SdkgenModule, out: &std::path::Path) -> Vec<Change> {
    let emitter = FileEmitter::new(lang());
    let ctx = EmitContext::new().with_output_dir(out);
    let changes = emitter.materialize(module, &ctx).expect("materialize");
    let store = FsStore::new(out);
    let engine = Engine::new(&store);
    let mut meta = RunMeta::new("test", "0.0.0");
    meta.new_run_id();
    engine
        .apply(&changes, None, &meta, &ApplyOptions::default())
        .expect("apply");
    changes
}

fn build_console_module() -> SdkgenModule {
    let mut module = SdkgenModule {
        module: "console".to_string(),
        backend_id: "webidl".to_string(),
        target: "browser".to_string(),
        client_package: "js".to_string(),
        library: "console".to_string(),
        source: Some("https://example.test/console.idl".to_string()),
        ..Default::default()
    };

    module.functions.push(Function {
        name: "console_log".to_string(),
        doc: None,
        port_name: "ConsoleLog".to_string(),
        port_args: vec![Arg {
            name: "message".to_string(),
            port: "String".to_string(),
        }],
        arg_descriptors: vec!["js_string".to_string()],
        return_port: "()".to_string(),
        ret_descriptor: "void".to_string(),
        out_struct: None,
        variadic: false,
        errno_guard: true,
        c_decl: "console.log".to_string(),
    });

    module
}

#[test]
fn emits_js_port_and_adapter() {
    let module = build_console_module();
    let tmp = PathBuf::from(format!(
        "/tmp/sdkgen_emit_webidl_test_{}",
        std::process::id()
    ));
    let port_dir = tmp.join("console");

    let _ = fs::remove_dir_all(&tmp);
    emit(&module, &port_dir);

    let port = fs::read_to_string(port_dir.join("console.hnl")).unwrap();
    // Adapters nest under `<port_pkg>/<adapter_pkg>/` inside the store
    // root — `ArtifactRef` cannot escape `req.output` (PL-004 T-075).
    let adapter =
        fs::read_to_string(port_dir.join("console-webidl-browser/console_c.hnl")).unwrap();

    assert!(
        port.contains("--- source: https://example.test/console.idl"),
        "port source pin missing: {port}"
    );
    assert!(
        adapter.contains("--- source: https://example.test/console.idl"),
        "adapter source pin missing: {adapter}"
    );
    assert!(
        port.contains("from \"js\" import { JsError }"),
        "port import missing: {port}"
    );
    assert!(
        port.contains("export ConsoleLog:"),
        "port arrow missing: {port}"
    );
    assert!(
        port.contains("~[1]~>"),
        "1-morphism missing in port: {port}"
    );
    assert!(
        port.contains("Result<(), JsError>"),
        "JS result type missing in port: {port}"
    );

    assert!(
        adapter.contains("from \"js\" import { call, error }"),
        "adapter import missing: {adapter}"
    );
    assert!(
        adapter.contains("export console_log: ConsoleLog :="),
        "adapter arrow missing: {adapter}"
    );
    assert!(
        adapter.contains("call(\"console\", \"log\""),
        "JS call missing: {adapter}"
    );
    assert!(
        adapter.contains("\"js_string\""),
        "js_string descriptor missing: {adapter}"
    );
    assert!(
        adapter.contains("\"js_void\""),
        "js_void descriptor missing: {adapter}"
    );
    assert!(
        adapter.contains("e := error()"),
        "error() call missing: {adapter}"
    );
    assert!(adapter.contains("err(e)"), "err(e) missing: {adapter}");

    // Emitted files are canonical: `hnl fmt --check` must pass on both.
    assert!(
        handl_plugin_ext_fmt::check_source_with_language(&lang(), &port).unwrap_or(false),
        "port is not canonical:\n{port}"
    );
    assert!(
        handl_plugin_ext_fmt::check_source_with_language(&lang(), &adapter).unwrap_or(false),
        "adapter is not canonical:\n{adapter}"
    );

    let _ = fs::remove_dir_all(&tmp);
}

/// `per_record_files` emits one `<Record>.hnl` port / `<Record>_c.hnl`
/// adapter pair per record plus `types.hnl`/`types_c.hnl` for typedefs
/// and loose functions — the single-file `<module>.hnl` port and
/// `<module>_c.hnl` adapter are not written.
#[test]
fn emits_per_record_layout() {
    let mut module = build_console_module();
    module.per_record_files = true;
    module.records.push(Record {
        name: "Widget".into(),
        fields: vec![],
        size: 0,
        align: 0,
        doc: None,
    });
    module.functions.push(Function {
        name: "widget_find".to_string(),
        doc: None,
        port_name: "WidgetFind".to_string(),
        port_args: vec![],
        arg_descriptors: vec![],
        return_port: "()".to_string(),
        ret_descriptor: "void".to_string(),
        out_struct: None,
        variadic: false,
        errno_guard: true,
        c_decl: "Widget.find".to_string(),
    });
    module.typedefs.push(Typedef {
        name: "WidgetInit".into(),
        port: "Handle".into(),
        descriptor: "js_object".into(),
        doc: None,
    });

    let tmp = PathBuf::from(format!(
        "/tmp/sdkgen_emit_per_record_test_{}",
        std::process::id()
    ));
    let port_dir = tmp.join("console");
    // Adapters nest under `<port_pkg>/<adapter_pkg>/` inside the store
    // root — `ArtifactRef` cannot escape `req.output` (PL-004 T-075).
    let adapter_dir = port_dir.join("console-webidl-browser");

    let _ = fs::remove_dir_all(&tmp);
    let changes = emit(&module, &port_dir);

    // The single-file `<module>.hnl` port and `<module>_c.hnl`/legacy
    // `<module>.hnl` adapter artifacts are declared as `Change::Delete`
    // so a regenerated spec doesn't leave stale files behind.
    let deleted: Vec<&str> = changes
        .iter()
        .filter_map(|c| match c {
            Change::Delete { artifact } => Some(artifact.as_str()),
            _ => None,
        })
        .collect();
    assert!(deleted.contains(&"console.hnl"), "{deleted:?}");
    assert!(
        deleted.contains(&"console-webidl-browser/console_c.hnl"),
        "{deleted:?}"
    );
    assert!(
        deleted.contains(&"console-webidl-browser/console.hnl"),
        "{deleted:?}"
    );

    let widget_port = fs::read_to_string(port_dir.join("Widget.hnl")).unwrap();
    let types_port = fs::read_to_string(port_dir.join("types.hnl")).unwrap();
    let widget_adapter = fs::read_to_string(adapter_dir.join("Widget_c.hnl")).unwrap();
    let types_adapter = fs::read_to_string(adapter_dir.join("types_c.hnl")).unwrap();

    // Neither the legacy same-named adapter nor the `_c` single-file
    // artifact may linger after a per-record emission.
    assert!(!adapter_dir.join("console.hnl").exists());
    assert!(!adapter_dir.join("console_c.hnl").exists());
    assert!(!port_dir.join("console.hnl").exists());

    assert!(widget_port.contains("export Widget :="), "{widget_port}");
    assert!(widget_port.contains("export WidgetFind:"), "{widget_port}");
    assert!(types_port.contains("export WidgetInit"), "{types_port}");
    // The loose `console.log` function lands in types.hnl.
    assert!(types_port.contains("export ConsoleLog:"), "{types_port}");

    assert!(
        widget_adapter.contains("from \"../Widget\" import { Widget, WidgetFind }"),
        "{widget_adapter}"
    );
    assert!(
        types_adapter.contains("from \"../types\" import { ConsoleLog }"),
        "{types_adapter}"
    );

    let _ = fs::remove_dir_all(&tmp);
}
