# Dependency rationale

Direct dependencies serve the boundaries below. Internal COMSAT crates follow the
[architecture](architecture.md). `cargo deny`, `cargo machete`, and the architecture
validator run in the canonical gate; adding a dependency must retain the relevant
native and WASM checks.

| Dependency | Reason |
| --- | --- |
| `incurs` | Canonical typed command graph, ToolCatalog, MCP, HTTP, and Agent Plugins |
| `incurs-codemode` | Existing composition lifecycle and tool connector |
| `incurs-codemode-local` | Existing JavaScript executor; no COMSAT interpreter |
| `incurs-mcp-cloudflare` | Existing Worker-compatible MCP transport |
| `serde`, `serde_json` | Canonical serialization, source JSON, and JSONL interoperability |
| `schemars` | Derive tool schemas from the same Rust data contracts |
| `thiserror` | Consistent typed errors across source and persistence boundaries |
| `url` | Standards-based URL parsing and source-target validation |
| `time` | RFC 3339 parsing and comparison without string-order assumptions |
| `http` | Transport-neutral typed HTTP requests and responses |
| `percent-encoding` | Correct query component encoding for upstream APIs |
| `reqwest` | Native TLS, bounded response streaming, and required API decompression |
| `worker` | Official Rust bindings for Workers, D1, Queue, Cron, and cancellation |
| `rusqlite` | Native transactional SQLite; bundled SQLite keeps self-hosting portable |
| `futures` | Runtime-neutral streams and bounded fan-out coordination |
| `async-stream` | Progressive source and canonical record streams without manual state machines |
| `async-trait` | Object-safe asynchronous substitution at actual source/store boundaries |
| `tokio`, `tokio-util` | Native execution, bounded I/O, and Incurs-compatible cancellation |
| `web-time` | Native and WASM-compatible source cooldown clocks |
| `syn` | Parse Rust syntax for executable architecture/complexity checks |
| `proc-macro2`, `quote` | Preserve syntax locations and inspect parsed Rust tokens in quality checks |
| `toml` | Parse Cargo manifests for dependency-boundary enforcement |

Incurs is pinned to Git revision `8b1a6c400b2eb65ee379e099c1d7bd96525f536a`.
The lockfile pins the complete application dependency resolution. Native SQLite,
network clients, and Cloudflare bindings are excluded from runtime-neutral type
contracts. The Cloudflare crate uses SQLite only in native development tests to
execute its production D1 SQL against the shared schema.
