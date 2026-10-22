# sdkgen-file

`handl-plugin-sdkgen-emitter-file` — the `hnl-file` emitter for the Handl SDK generator (ADR-0107): writes `SdkgenModule` IR as `.hnl` port/adapter package pairs. Serves the `c` and `webidl` targets, resolved by name through the `SdkgenRegistry`.

Sibling-repo deps are git dependencies (`branch = "main"`); `[patch]` tables
resolve them from sibling checkouts.

## Docs

- [docs/index.md](docs/index.md) — the `hnl-file` emitter: output
  layout, port/adapter emission rules, per-client call shapes,
  descriptor → port-arg contract.
- Framework: [`host/docs/sdkgen/`](https://github.com/handl-lang/host/blob/main/docs/sdkgen/index.md).
