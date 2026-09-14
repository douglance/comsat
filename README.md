# COMSAT

Composable search for humans and agents. Sources produce canonical records;
consumers decide what the evidence means.

COMSAT is a Rust domain layer over Incurs. Incurs owns command invocation, MCP,
Agent Plugin loading, and Code Mode. COMSAT owns source semantics, aggregation,
deduplication, watches, and information persistence.

```text
[Public sources: GitHub, Hacker News, Web, Stack Exchange]
                         | normalized records
                         v
[Incurs tools: schemas, invocation, cancellation]
                         | record chunks and source errors
                         v
[COMSAT engine: merge and deduplicate]
                         | canonical Record stream
                         v
[CLI / MCP / Code Mode: consumer bindings]
```

The managed runtime uses Rust Cloudflare Workers, D1, Queues, and Cron. The native
runtime uses SQLite. Both use the same source and information contracts. Local
queries require no COMSAT account.

Version 0.1.0 runs locally, as a self-hosted service, and on Cloudflare. The managed
HTTP/MCP service is deployed at <https://comsat-cloud.doug-lance.workers.dev> and
requires a bearer token. See the [acceptance ledger](docs/acceptance.md) for live
retrieval and scheduled-watch evidence, fixture coverage, and remaining v1 limits.

The repository pins its Rust toolchain and Incurs revision. The canonical quality
gate is `cargo xtask check`. Architecture and complexity checks are also callable
as `cargo xtask architecture` and `cargo xtask complexity`.

## Build and use

```sh
cargo build --release -p comsat-cli
target/release/comsat search "MCP OAuth" --source github --limit 5
target/release/comsat source list
target/release/comsat --mcp
```

Install the binary with `cargo install --path crates/comsat-cli --bin comsat`.
See [operations](docs/operations.md) for local persistence, managed deployment,
authentication, telemetry, and recovery.

Search, fetch, follow, and history emit canonical records as JSON Lines. Diagnostics
go to stderr. Search does not save observations. The native distribution includes
all four first-party sources; web search requires a configured provider key.

GitHub search covers issues and pull requests by default, repositories with
`type:repo`, and discussions with `type:discussion`; following a pull request
also returns its reviews and review comments, a repository returns its
discussions, and a discussion returns its comments.

Code Mode executions are durable. `comsat code run` persists its execution, and
`comsat code list`, `show`, `events`, `artifact`, `approve`, `reject`, `resume`,
`cancel`, `rollback`, and `prune` operate on that state from any later process.

Source credentials, supported operations, and bounded result limits are documented
in [sources](docs/sources.md). Set `COMSAT_PLUGINS` to a platform path list of Incurs
Agent Plugin directories to load additional sources.

`COMSAT_DATA_DIR` selects the native data directory. Otherwise COMSAT uses
`$XDG_DATA_HOME/comsat` or `$HOME/.local/share/comsat`. Watches explicitly persist
queries and observations; `serve` runs the scheduler and Incurs HTTP server in one
process. Tenant identity is selected by the runtime and is absent from public tool
arguments.

## Verification

```sh
cargo xtask check
```

The gate includes formatting, strict Clippy, fixture and integration tests,
dependency checks, executable architecture and complexity rules, source
conformance, separate WASM builds, and runtime smoke checks. Cloud runtime checks
use local Wrangler, D1, and Queues with temporary credentials and storage. They
require Node.js and build the Rust Worker with the pinned `worker-build` version.
Live provider and deployed-account acceptance are recorded separately.

Read the [architecture](docs/architecture.md) and
[third-party source guide](docs/plugins.md). All code and deployment adapters are
available under the [MIT license](LICENSE).
