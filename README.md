# tool-codegen-file

`handl-plugin-codegen-emitter-file` — the `hnl-file` emitter for the
Handl codegen pipeline: renders `SdkgenModule` IR as `.hnl`
port/adapter package pairs. Part of the `tool-codegen-*` family
(PL-004 T-075). Serves the `c` and `webidl` targets, resolved by name
through the `CodegenRegistry`.

## Development

```bash
cargo xtask gate        # fmt --check → clippy -D warnings → test → doc
```

`cargo xtask` wraps the repo's `xtask/` crate; `ci.sh` is the gate entry.
Deps resolve from git, so no sibling checkouts are needed; a plain clone
test-runs as-is.

## Docs

- [docs/index.md](docs/index.md) — the `hnl-file` emitter: output
  layout, port/adapter emission rules, per-client call shapes,
  descriptor → port-arg contract.
- Framework: [`host/crates/plugin-codegen`](https://github.com/handl-lang/host/tree/main/crates/plugin-codegen).

## License

MIT.
