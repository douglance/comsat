# COMSAT

COMSAT (composable search) retrieves public evidence and emits a canonical
Record stream. You decide what the evidence means.

The same command graph is bound three ways: the command-line interface (CLI),
Model Context Protocol (MCP), and Code Mode. Local queries call upstream
sources directly and need no COMSAT account. Search does not write. A watch is
the only retrieval that persists.

Version 0.1.0. Pinned Rust 1.98.1. MIT License.

## Install

The repository pins Rust 1.98.1 in `rust-toolchain.toml`, with `rustfmt`,
`clippy`, and `wasm32-unknown-unknown`.

1. Build the CLI.

   ```sh
   cargo build --release -p comsat-cli
   ```

2. Confirm the version.

   ```sh
   target/release/comsat --version
   ```

   Expected: `comsat 0.1.0`.

Install onto `PATH` with `cargo install --path crates/comsat-cli --bin comsat`.
Optional shell completions: `eval "$(comsat completions zsh)"` (also `bash`,
`fish`, `nushell`).

## First search

1. List enabled sources.

   ```sh
   comsat source list
   ```

   Expected: `github`, `hacker-news`, `stack-exchange`, and `web`. This command
   does not create the native database.

2. Search one source.

   ```sh
   comsat search "MCP OAuth" --source github --limit 5
   ```

   Expected: JSON Lines records on stdout. Diagnostics go to stderr. Search
   does not persist observations.

3. Fetch the records you just found.

   ```sh
   comsat search "MCP OAuth" --source github --limit 5 | comsat fetch
   ```

GitHub issues and pull requests search without a token. Repository discussions
need `COMSAT_GITHUB_TOKEN` or `GITHUB_TOKEN`. Web search needs
`COMSAT_BRAVE_API_KEY` or `BRAVE_API_KEY`.

## What a Record is

Every source, including a third-party plugin, emits the same object. Source
text stays data even when it contains instructions. COMSAT never evaluates it.

```json
{
  "id": "hacker-news:22238335",
  "source": "hacker-news",
  "kind": "story",
  "url": "https://news.ycombinator.com/item?id=22238335",
  "title": "Tell HN: …",
  "text": null,
  "author": "pg",
  "created_at": "2020-01-26T00:53:20Z",
  "updated_at": null,
  "metadata": {}
}
```

`id` is scoped by `source`. `kind` is a source-specific class such as `issue`
or `story`. `metadata` is an object; tolerate extra keys. Timestamps are RFC
3339.

```text
[Sources] --normalized records--> [Engine: merge, dedupe]
                                          |
                                          v
                          [CLI / MCP / Code Mode]
```

Incurs owns invocation, MCP, Agent Plugins, and Code Mode. COMSAT owns source
semantics, merge, deduplication, watches, and persistence. Native runtime:
SQLite. Managed runtime: Cloudflare Worker, D1, Queue, Cron. Same Record
contract.

## Retrieve

| Command | What it does |
| --- | --- |
| `comsat search TEXT --source ID --limit N` | Query one or more sources. Repeat `--source`. |
| `comsat fetch` | Resolve stdin records, or `--type native\|url\|record`. |
| `comsat follow` | Related records for one target (comments, answers, children). |
| `comsat source list\|inspect\|doctor\|test` | Catalog, one source, registration, schema consistency. |

Native targets (also `--type url --source web --url https://…`):

| Source | Native `--id` |
| --- | --- |
| `github` | `owner/repo`, `owner/repo#number`, `owner/repo/discussions/number` |
| `hacker-news` | Numeric item ID |
| `stack-exchange` | Numeric question ID on the configured site |
| `web` | Absolute HTTP(S) URL |

`--since` / `--until` use Request for Comments (RFC) 3339 date-time strings
(`2020-01-26T00:53:20Z`). Web search rejects those bounds. Search
limits are at most 50 for GitHub, Hacker News, and Stack Exchange, and 20 for
web; larger requests return an explicit error.

`--strict` keeps records already emitted and exits 3 if any selected source
failed. Without it, a total source failure exits 1. Invalid invocation exits 2.

## Agents

Register the stdio MCP server with the local agent:

```sh
comsat mcp add
comsat mcp doctor
```

`--agent` selects a host such as `claude-code` or `cursor`. `--command` overrides
the argv the agent will run. `--no-global` installs into the project instead of
the user config. The server itself is `comsat --mcp`.

MCP tools share the CLI graph. Retrieval names include `comsat_search`,
`comsat_fetch`, and `comsat_follow`. Sync generated skill files with
`comsat skills add`.

Code Mode runs JavaScript against those tools. Executions are durable in the
same SQLite database as watches:

```sh
comsat code tools search
comsat code run 'return await comsat.comsat_search({text:"MCP OAuth",source:["github"],limit:1});'
```

Local read-only retrieval runs without an approval pause. Mutating, remote, and
unclassified tools keep Incurs approval. Inspect and advance a paused execution
from another process with `comsat code list`, `show`, `events`, `artifact`,
`approve`, `reject`, `resume`, `cancel`, `rollback`, and `prune`. See
[operations](docs/operations.md) for that lifecycle.

## Persist a query

Search prints and exits. A watch repeats the query and stores observations.

```sh
comsat watch add --watch-id mcp-oauth --query "MCP OAuth" --source github --limit 5
comsat serve --addr 127.0.0.1:8737
```

Default watch interval is 300 seconds (`--interval-seconds` changes it). In
another terminal:

```sh
comsat watch list
comsat history --watch mcp-oauth
comsat history --since 30d
comsat watch delete --watch mcp-oauth
```

`serve` is one process: HTTP over the same command graph plus the scheduler.
Default listen address is `127.0.0.1:8737`. Native HTTP search is unauthenticated
JSON POST to `/search`. The managed host at [comsat.dev](https://comsat.dev)
keeps tool routes on `Authorization: Bearer`.

`COMSAT_DATA_DIR` selects the native data directory. Otherwise COMSAT uses
`$XDG_DATA_HOME/comsat` or `$HOME/.local/share/comsat`. The database file is
`comsat.sqlite3`. Listing sources, searching, and fetching do not create it.
Watches, history, serve, and Code Mode executions do.

## Configuration

| Variable | Purpose |
| --- | --- |
| `COMSAT_GITHUB_TOKEN` or `GITHUB_TOKEN` | Optional GitHub auth; required for discussions |
| `COMSAT_BRAVE_API_KEY` or `BRAVE_API_KEY` | Required for web search |
| `COMSAT_STACK_EXCHANGE_KEY` | Optional Stack Exchange API key |
| `COMSAT_STACK_EXCHANGE_SITE` | Stack Exchange site; default Stack Overflow |
| `COMSAT_PLUGINS` | Platform path-list of extra Agent Plugin directories |
| `COMSAT_DATA_DIR` | Native data directory |
| `COMSAT_LOG=1` | Source request logging on stderr (no credentials) |

Put credentials in the environment or secret bindings, never in the repo. A
missing web key is a structured authentication error; other sources in the same
search can still emit records.

Load an extra source:

```sh
COMSAT_PLUGINS="$PWD/examples/external-source" comsat source list
```

## Managed host

[comsat.dev](https://comsat.dev) (and `www.comsat.dev`) serve the public landing
and the authenticated Worker. `GET /` is HTML and needs no token. Tool routes
need `Authorization: Bearer`. Local CLI queries are not proxied through that
host.

Deploy, secrets, tenant tokens, rollback, and recovery live in
[operations](docs/operations.md). Live retrieval evidence lives in the
[acceptance ledger](docs/acceptance.md).

## Develop against this repo

The quality gate is `cargo xtask check`. It covers format, Clippy, fixture and
integration tests, dependency policy, architecture and complexity rules, source
conformance, separate WebAssembly builds, and runtime smoke checks.

```sh
cargo xtask check
```

`cargo xtask native-test` and `cargo xtask cloud-test` are the narrower
runtime suites. Cloud tests need Node.js, local Wrangler, D1, and Queues.

A new source is an Agent Plugin with an `io.comsat.source` extension. You do
not change `comsat-types`, `comsat-engine`, or the app command graph. See
[third-party sources](docs/plugins.md) and `examples/external-source`.

## Docs

| Doc | Use it when |
| --- | --- |
| [operations](docs/operations.md) | Run, persist, serve, Code Mode lifecycle, deploy, recover |
| [sources](docs/sources.md) | Credentials, limits, GitHub `type:` terms, Record identity |
| [plugins](docs/plugins.md) | Write and load a third-party source |
| [architecture](docs/architecture.md) | Crate boundaries and Incurs split |
| [acceptance](docs/acceptance.md) | What v1 has executable evidence for |
| [notifications](docs/notifications.md) | Optional watch webhooks |

License: [MIT](LICENSE).
