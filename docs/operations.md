# Running COMSAT

## Native use

Build with the repository's pinned Rust toolchain:

```sh
cargo build --release -p comsat-cli
target/release/comsat search "MCP OAuth" --source github --limit 5
target/release/comsat search "MCP OAuth" --source github --limit 5 | target/release/comsat fetch
```

Search, fetch, follow, and history use JSONL. Diagnostics use stderr. Source
selection is repeatable. `--strict` signals a source failure with exit code 3
and preserves records already emitted. Invalid invocations use exit code 2.

For agent access, start `comsat --mcp`. The MCP tools derive from the same Incurs
command graph. Inspect local Code Mode capabilities with `comsat code tools search`.

```sh
comsat code run 'return await comsat.comsat_search({text:"MCP OAuth",source:["github"],limit:1});'
```

Code Mode runs JavaScript through the existing Incurs local executor. It exposes
tool results as data. It does not evaluate source text as instructions. Local
read-only retrieval tools execute without an approval pause. Mutating, remote,
and unclassified tools retain Incurs approval requirements. A paused, failed, or
cancelled execution returns nonzero with its structured state; only a completed
execution returns zero.

Executions are durable. They are stored in the same SQLite database as watches
and history, so an execution paused for approval in one process is inspected and
advanced from another:

| Command | Purpose |
| --- | --- |
| `comsat code list` | Durable executions, newest first |
| `comsat code show <execution>` | One execution snapshot with artifact references |
| `comsat code events <execution>` | Retained lifecycle and streaming events |
| `comsat code artifact <execution> <artifact>` | One oversized value owned by an execution |
| `comsat code approve <execution> <seq>` | Approve a pending action and continue |
| `comsat code reject <execution> <seq>` | Reject a pending action |
| `comsat code resume <execution>` | Replay a paused execution deterministically |
| `comsat code cancel <execution>` | Cancel a running or paused execution |
| `comsat code rollback <execution>` | Compensate applied actions in reverse order |
| `comsat code prune <keep>` | Keep the newest executions and delete the rest |

Rollback compensates `watch_add` by deleting the watch it created. Any other
applied action reports that it did not compensate rather than claiming a revert
it cannot perform. Reading Code Mode state never creates the database.

## Self-hosted watches

```sh
comsat watch add --watch-id mcp-oauth --query "MCP OAuth" --source github --limit 5
comsat serve --addr 127.0.0.1:8737
```

In another terminal, use `comsat history --watch mcp-oauth` or
`comsat history --since 30d`. `comsat watch delete --watch mcp-oauth` removes the
watch. The default watch interval is 300 seconds; `--interval-seconds` changes it.

The server uses one process and SQLite. `COMSAT_DATA_DIR` selects the database
directory. The default is `$XDG_DATA_HOME/comsat` or `$HOME/.local/share/comsat`.
Reading sources does not create the database or persist results.

## Managed deployment

The checked-in configuration targets the user's `doug-lance` Cloudflare profile.
For another account, change `account_id` and create its D1 database and Queue.
The Worker is the origin for `comsat.dev` and `www.comsat.dev`; the
`workers.dev` hostname remains as a fallback. Cloud application code, schema,
and deployment configuration use the same MIT license as the native
implementation.

```text
[Cron: find due watches] --claimed work--> [Queue: distribute runs]
                                                  |
                                                query
                                                  v
[HTTP/MCP: authenticated tenant] --query--> [Engine: retrieve records]
                                                  |
                                               records
                                                  v
                                         [D1: fenced persistence]
```

Use the pinned Wrangler version and the named account profile:

```sh
npx --yes wrangler@4.131.1 auth activate doug-lance "$PWD"
npx --yes wrangler@4.131.1 d1 migrations apply comsat --remote --config crates/comsat-cloud/wrangler.jsonc
npx --yes wrangler@4.131.1 deploy --config crates/comsat-cloud/wrangler.jsonc --secrets-file "$HOME/.config/comsat/cloud-secrets.json"
```

Custom domains are declared in `crates/comsat-cloud/wrangler.jsonc`. To attach or
detach them without uploading a new Worker version:

```sh
npx --yes wrangler@4.131.1 triggers deploy --config crates/comsat-cloud/wrangler.jsonc
```

`worker-build` 0.8.5 must be installed. The build configuration preserves WASM
symbol information needed by wasm-bindgen while stripping debug information.
Do not replace `CARGO_PROFILE_RELEASE_STRIP=debuginfo` with full symbol stripping.

The secrets file is a private JSON object whose `TENANT_TOKENS_JSON` value is a
JSON-encoded mapping from random bearer tokens to tenant IDs. Keep it outside the
checkout with mode 0600. A caller cannot choose another tenant through tool
arguments. Rotate a token by replacing its entry and uploading the updated secret.

| Cloud binding | Meaning |
| --- | --- |
| `COMSAT_DB` | D1 database |
| `COMSAT_WATCH_QUEUE` | Queue for claimed watch runs |
| `TENANT_TOKENS_JSON` | Required secret mapping bearer tokens to tenant IDs |
| `MCP_ALLOWED_ORIGINS` | Explicit browser origin allowlist; empty allows headless requests only |
| `WATCH_LEASE_SECONDS` | Queue claim lease, with a 300-second minimum |
| `GITHUB_TOKEN` | Optional GitHub credential |
| `BRAVE_SEARCH_API_KEY` | Required for managed web search |
| `STACK_EXCHANGE_API_KEY` | Optional Stack Exchange credential |
| `STACK_EXCHANGE_SITE` | Site selection; defaults to Stack Overflow |
| `TENANT_WEBHOOKS_JSON` | Optional per-tenant HTTPS notification configuration |

`GET /` is a public HTML explanation of the Record pipeline and does not require a
token. Authenticated endpoints include `/mcp`, `/search`, `/fetch`, `/follow`,
`/watch/add`, `/watch/list`, `/watch/delete`, and `/history`. Tool endpoints accept
POST JSON arguments. History accepts GET query parameters. `/health` also requires
authentication. MCP protocol negotiation is supplied by the pinned Incurs adapter.

Cloud retrieval permits four active sources and at most 240 KiB of normalized
records per source per operation. A web page larger than that budget is refused
with an `unsupported` source error naming the budget, because the source
answered correctly and the limit belongs to this deployment. The native runtime
applies no such ceiling and truncates a fetched page at 256 KB instead. Budget exhaustion is a structured source error.
D1 watch completion accepts at most 1 MiB of serialized record data and uses seven
transactional SQL statements, or eight with an atomic notification batch. Every
write is fenced by tenant, run, and lease. Scheduled watch claims are capped at 12
per invocation; tenant polling rotates and is bounded even when no watches are due.
Cron runs every five minutes; Queue delivery may occur later. Source cooldowns
persist within a Worker isolate; upstream quotas still apply across isolates.

## Verification and recovery

Run `cargo xtask check` before integration. `cargo xtask native-test` exercises the
CLI and self-hosted scheduler using the external fixture source. `cargo xtask
cloud-test` exercises an actual local Worker, D1, Queue, MCP, authentication, and
tenant isolation. Live upstream and deployed watch evidence is tracked separately
in the [acceptance ledger](acceptance.md).

Cloudflare observability captures Worker logs and failures. Native server source
and watch diagnostics are structured stderr events; `COMSAT_LOG=1` enables source
request logging for direct commands. Logs exclude source credentials.

Optional [watch notifications](notifications.md) use a transactional outbox and
bounded HTTPS retries. They are disabled until explicitly configured.

`source_metrics` reports cumulative operation requests, successes, failures, rate
limits, cancellations, emitted records, and duplicates. Worker snapshots carry an
isolate ID; native snapshots carry a process ID. Compute deltas within that ID;
do not sum successive cumulative snapshots. Restarting a process or isolate resets
its counters. `source_http_request` measures elapsed time through response-body
completion and reports status or a structured error class without request URLs.

Each managed scheduled invocation logs enqueued work and a D1 storage snapshot:
active tenants/watches, due and leased watches, stored records, record-content
bytes, watch runs/failures, and notification failures. Record-content bytes measure
text, titles, and metadata, not physical database size. Compare snapshots to measure
growth. Physical D1 size, Worker failures, and actual Queue backlog come from
Cloudflare's platform metrics; due/leased watch counts are separate measurements.

Use Wrangler deployment history and rollback for a Worker regression. Keep the
database when rolling code back; do not delete observations to repair an execution
failure. Code rollback does not remove custom domains. To detach `comsat.dev`,
delete the `routes` entries and run `wrangler triggers deploy`. Expired watch
leases become claimable again, and run idempotency prevents a completed delivery
from being committed twice. Obsolete or invalid Queue jobs are acknowledged;
database failures request a Queue retry.
