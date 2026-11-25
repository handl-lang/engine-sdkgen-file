# Agent guide — tool-codegen-file

`handl-plugin-codegen-emitter-file` — the `hnl-file` emitter for the
Handl codegen pipeline: renders `SdkgenModule` IR as `.hnl`
port/adapter package pairs. Part of the `tool-codegen-*` family
(PL-004 T-075). Serves the `c` and `webidl` targets, resolved by name
through the `CodegenRegistry`. Rust 2024.

## Layout

| path | role |
| --- | --- |
| `src/lib.rs` | the whole emitter — port/adapter `.hnl` rendering |
| `tests/` | `emit_clock`, `emit_webidl`, `emit_worker_webidl` + `fixtures/` |
| `docs/` | emitter doc — output layout, port/adapter rules, per-client call shapes |
| `xtask/`, `workflows/gate.yaml`, `bupha.toml`, `.cargo/` | vendored repo gate + CI wiring |

## Contract

- Deps resolve from git (`handl-plugin-codegen`, `handl-plugin-ext-fmt`
  → `handl-lang/host`; `handl-language` → `handl-lang/kernel`), except
  `handl-plugin-codegen-backend-webidl` which is a **path dep** on
  `../tool-codegen-webidl` (backend marks js dispatch forms
  `recv:`/`handler:`/`constructor`).
- Output is `.hnl` source pairs — emission is text, verified by the
  golden-style emit tests.

## Verification

```bash
cargo xtask gate   # fmt --check → clippy -D warnings → test → doc
```


<!-- myco:agent-awareness -->

## Agent awareness

- Session start / any work discussion: run `myco status --prompt`
  before proposing next steps — know the board first.
- User describes new work → offer to capture it (`myco todo add`,
  `issue new`, `proposal new`). Don't wait to be asked.
- Picking up a task: `myco resume <ID>` is the briefing.
- Wrapping up: `myco handoff <ID>` anything resumable; keep statuses
  honest (claimed → in-progress → blocked/done).

<!-- /myco:agent-awareness -->
