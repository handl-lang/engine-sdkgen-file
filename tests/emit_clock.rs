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
/// (`FsStore` + `Engine::apply`) and return the `Change` set — the
/// same path `CodegenRegistry::run` drives (PL-004).
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

fn build_clock_module() -> SdkgenModule {
    let mut module = SdkgenModule {
        module: "clock".to_string(),
        backend_id: "c".to_string(),
        target: "x86_64-unknown-linux".to_string(),
        client_package: "libc".to_string(),
        library: "libc.so.6".to_string(),
        ..Default::default()
    };

    module.records.push(Record {
        name: "Timespec".to_string(),
        doc: None,
        fields: vec![
            Field {
                name: "tv_sec".to_string(),
                port: "Int".to_string(),
                descriptor: "i64".to_string(),
                offset: 0,
                size: 8,
                ptr_depth: 0,
                is_const: false,
                doc: None,
            },
            Field {
                name: "tv_nsec".to_string(),
                port: "Int".to_string(),
                descriptor: "i64".to_string(),
                offset: 8,
                size: 8,
                ptr_depth: 0,
                is_const: false,
                doc: None,
            },
        ],
        size: 16,
        align: 8,
    });

    module.functions.push(Function {
        name: "clock_gettime".to_string(),
        doc: None,
        port_name: "ClockGettime".to_string(),
        port_args: vec![
            Arg {
                name: "clock_id".to_string(),
                port: "Int".to_string(),
            },
            Arg {
                name: "tp".to_string(),
                port: "Timespec".to_string(),
            },
        ],
        arg_descriptors: vec![
            "i32".to_string(),
            "out_struct:{tv_sec:i64@0,tv_nsec:i64@8}".to_string(),
        ],
        return_port: "ClockGettimeResult".to_string(),
        ret_descriptor: "i32".to_string(),
        out_struct: Some("Timespec".to_string()),
        variadic: false,
        errno_guard: true,
        c_decl: "int clock_gettime(clockid_t clock_id, struct timespec *tp);".to_string(),
    });

    module
}

#[test]
fn emits_port_and_adapter() {
    let module = build_clock_module();
    let tmp = PathBuf::from(format!("/tmp/sdkgen_emit_test_{}", std::process::id()));
    let port_dir = tmp.join("libc");

    let _ = fs::remove_dir_all(&tmp);
    emit(&module, &port_dir);

    let port = fs::read_to_string(port_dir.join("clock.hnl")).unwrap();
    // Adapters nest under `<port_pkg>/<adapter_pkg>/` inside the store
    // root — `ArtifactRef` cannot escape `req.output` (PL-004 T-075).
    let adapter = fs::read_to_string(port_dir.join("libc-c-linux-x86_64/clock_c.hnl")).unwrap();

    assert!(
        port.contains("export Timespec := {"),
        "port record missing: {port}"
    );
    assert!(
        port.contains("export ClockGettime:"),
        "port arrow missing: {port}"
    );
    // out_struct keeps `ret` — the port declares a Result record whose
    // `buf` is the struct, and the adapter returns it whole.
    assert!(
        port.contains("export ClockGettimeResult := {"),
        "result record missing: {port}"
    );
    assert!(
        port.contains("buf: Timespec"),
        "result buf field missing: {port}"
    );
    assert!(
        adapter.contains("export clock_gettime: ClockGettime :="),
        "adapter missing: {adapter}"
    );
    assert!(
        adapter.contains("call(\"libc.so.6\""),
        "call missing: {adapter}"
    );
    assert!(
        adapter.contains("err(ffi.errno())"),
        "err(ffi.errno()) missing: {adapter}"
    );
    assert!(adapter.contains("ok(r)"), "ok(r) missing: {adapter}");

    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn out_buf_descriptor_gets_buf_unwrap_body() {
    let mut module = SdkgenModule {
        module: "socket".to_string(),
        backend_id: "c".to_string(),
        target: "x86_64-unknown-linux".to_string(),
        client_package: "libc".to_string(),
        library: "libc.so.6".to_string(),
        ..Default::default()
    };

    module.functions.push(Function {
        name: "recv".to_string(),
        doc: None,
        port_name: "Recv".to_string(),
        port_args: vec![
            Arg {
                name: "fd".to_string(),
                port: "Int".to_string(),
            },
            Arg {
                name: "flags".to_string(),
                port: "Int".to_string(),
            },
        ],
        arg_descriptors: vec![
            "i32".to_string(),
            "out_buf:65536".to_string(),
            "i32".to_string(),
        ],
        return_port: "Obj".to_string(),
        ret_descriptor: "i64".to_string(),
        out_struct: None,
        variadic: false,
        errno_guard: true,
        c_decl: "ssize_t recv(int, void *, size_t, int);".to_string(),
    });

    let tmp = PathBuf::from(format!("/tmp/sdkgen_emit_outbuf_{}", std::process::id()));
    let port_dir = tmp.join("libc");

    let _ = fs::remove_dir_all(&tmp);
    emit(&module, &port_dir);

    let adapter = fs::read_to_string(port_dir.join("libc-c-linux-x86_64/socket_c.hnl")).unwrap();
    assert!(
        adapter.contains("\"out_buf:65536\""),
        "out_buf descriptor missing: {adapter}"
    );
    assert!(
        adapter.contains("if r.ret < 0"),
        "r.ret check missing: {adapter}"
    );
    assert!(
        adapter.contains("ok(r.buf)"),
        "ok(r.buf) missing: {adapter}"
    );

    let _ = fs::remove_dir_all(&tmp);
}
