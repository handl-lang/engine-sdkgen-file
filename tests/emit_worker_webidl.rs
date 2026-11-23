//! End-to-end WebIDL → `.hnl` emission for the Worker-class surface:
//! the backend's dispatch markers (`recv:`/`handler:`/`constructor`)
//! must land as `js.invoke`/`js.on`/`js.construct` adapter calls.
//!
//! The spec is inlined (a trimmed `html.idl` Worker slice) so the test
//! is self-contained — no sibling checkout needed.

use std::fs;
use std::path::PathBuf;

use handl_plugin_sdkgen::{Emitter, SdkgenBackend};
use handl_plugin_sdkgen_backend_webidl::WebIdlBackend;
use handl_plugin_sdkgen_emitter_file::FileEmitter;

/// The canonical language, assembled from the dev-dep data crate.
fn lang() -> std::sync::Arc<handl_language::Language> {
    std::sync::Arc::new(
        handl_kit_data_prelude::language_from_yaml()
            .expect("canonical language assembles"),
    )
}

const WORKER_IDL: &str = r#"
callback EventHandlerNonNull = any (Event event);
typedef EventHandlerNonNull? EventHandler;

callback OnErrorEventHandlerNonNull = any ((Event or DOMString) event, optional DOMString source, optional unsigned long lineno, optional unsigned long colno, optional any error);
typedef OnErrorEventHandlerNonNull? OnErrorEventHandler;

interface EventTarget {
  undefined addEventListener(DOMString type, EventListener? callback, optional AddEventListenerOptions options = {});
  undefined removeEventListener(DOMString type, EventListener? callback, optional EventListenerOptions options = {});
  boolean dispatchEvent(Event event);
};

interface mixin MessageEventTarget {
  attribute EventHandler onmessage;
  attribute EventHandler onmessageerror;
};

interface mixin AbstractWorker {
  attribute EventHandler onerror;
};

[Exposed=(Window,Worker)]
interface Worker : EventTarget {
  constructor((TrustedScriptURL or USVString) scriptURL, optional WorkerOptions options = {});

  undefined terminate();

  undefined postMessage(any message, sequence<object> transfer);
  undefined postMessage(any message, optional StructuredSerializeOptions options = {});

  attribute DOMString label;

  static undefined register(USVString name);
};

Worker includes AbstractWorker;
Worker includes MessageEventTarget;

dictionary WorkerOptions {
  DOMString name = "";
  WorkerType type = "classic";
};

enum WorkerType { "classic", "module" };

dictionary StructuredSerializeOptions {
  sequence<object> transfer = [];
};
"#;

#[test]
fn emits_worker_dispatch_forms() {
    let tmp = PathBuf::from(format!(
        "/tmp/sdkgen_emit_worker_test_{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    let idl_path = tmp.join("worker.idl");
    fs::write(&idl_path, WORKER_IDL).unwrap();

    let backend = WebIdlBackend::new(None);
    let module = backend.parse_header(&idl_path).expect("parse worker.idl");
    assert!(module.diagnostics.is_empty(), "{:?}", module.diagnostics);

    let port_dir = tmp.join("worker");
    FileEmitter::new(lang()).emit(&module, &port_dir).unwrap();

    let adapter_dir = tmp.join("worker-webidl-browser");
    let worker_port = fs::read_to_string(port_dir.join("Worker.hnl")).unwrap();
    let worker_adapter = fs::read_to_string(adapter_dir.join("Worker_c.hnl")).unwrap();

    // The adapter imports exactly the ops the dispatch forms need.
    assert!(
        worker_adapter.contains("from \"js\" import { call, invoke, construct, on, error }"),
        "js import line missing: {}",
        worker_adapter.lines().next().unwrap_or("")
    );

    // Constructor → js.construct on the global `Worker`.
    assert!(
        worker_adapter.contains("r = construct(\"\", \"Worker\", [scriptURL, options]"),
        "construct call missing:\n{worker_adapter}"
    );

    // Instance ops → js.invoke with `self` as receiver.
    assert!(
        worker_adapter.contains("r = invoke(self, \"terminate\", [], [], \"js_void\", false)"),
        "invoke(terminate) missing:\n{worker_adapter}"
    );
    // The later `postMessage` overload wins the shared port name.
    assert!(
        worker_adapter.contains("r = invoke(self, \"postMessage\", [message, options]"),
        "invoke(postMessage) missing:\n{worker_adapter}"
    );

    // Static op → js.call on the `Worker` global (no receiver).
    assert!(
        worker_adapter.contains("r = call(\"Worker\", \"register\", [name]"),
        "call(register) missing:\n{worker_adapter}"
    );

    // `on<event>` attributes → js.on registrations.
    assert!(
        worker_adapter.contains("r = on(self, \"message\", handler)"),
        "on(message) missing:\n{worker_adapter}"
    );
    assert!(
        worker_adapter.contains("r = on(self, \"messageerror\", handler)"),
        "on(messageerror) missing:\n{worker_adapter}"
    );
    assert!(
        worker_adapter.contains("r = on(self, \"error\", handler)"),
        "on(error) missing:\n{worker_adapter}"
    );

    // The `on` result convention: null + recorded error is the failure.
    assert!(
        worker_adapter.contains("if r == null && e != null"),
        "on-result guard missing:\n{worker_adapter}"
    );

    // Port signatures carry the receiver/handler args.
    assert!(
        worker_port.contains("export WorkerTerminate: (Handle) ~[1]~>"),
        "port signature missing receiver:\n{worker_port}"
    );
    assert!(
        worker_port.contains("export WorkerOnmessage: (Handle, Handle) ~[1]~>"),
        "port onmessage signature missing:\n{worker_port}"
    );
    assert!(
        worker_port.contains("export WorkerConstructor: (Handle, Handle) ~[1]~> Result<Handle, JsError>"),
        "port constructor must return the instance:\n{worker_port}"
    );

    // Golden artifact: `SDKGEN_WORKER_GOLDEN=<dir>` copies the emitted
    // port/adapter pair there — regenerate the committed fixtures with
    // `SDKGEN_WORKER_GOLDEN=tests/fixtures/worker cargo test
    // --test emit_worker_webidl`.
    if let Ok(golden) = std::env::var("SDKGEN_WORKER_GOLDEN") {
        let golden = PathBuf::from(golden);
        fs::create_dir_all(&golden).unwrap();
        fs::copy(port_dir.join("Worker.hnl"), golden.join("Worker.hnl")).unwrap();
        fs::copy(
            adapter_dir.join("Worker_c.hnl"),
            golden.join("Worker_c.hnl"),
        )
        .unwrap();
    }

    // Emitted files are canonical: `hnl fmt --check` must pass.
    for f in [
        worker_port.as_str(),
        worker_adapter.as_str(),
    ] {
        assert!(
            handl_plugin_ext_fmt::check_source_with_language(&lang(), f).unwrap_or(false),
            "emitted source is not canonical:\n{f}"
        );
    }

    let _ = fs::remove_dir_all(&tmp);
}
