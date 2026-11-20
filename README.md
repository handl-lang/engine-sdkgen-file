# sdkgen-file

`handl-plugin-sdkgen-emitter-file` — the `hnl-file` emitter for the Handl
SDK generator: writes `SdkgenModule` IR as `.hnl` port/adapter package
pairs. Part of the `sdkgen-*` family. Serves the `c` and `webidl`
targets, resolved by name through the `SdkgenRegistry`.

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
- Framework: [`host/docs/sdkgen/`](https://github.com/handl-lang/host/blob/main/docs/sdkgen/index.md).

## License

MIT.
