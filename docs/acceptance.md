# V1 acceptance ledger

Status: version 0.1.0 is implemented and deployed. The evidence below was collected
on September 13, 2026. This ledger does not certify every v1 requirement complete.
Apoc execution IDs refer to durable local command receipts and their output.

| Scenario | Evidence | Result |
| --- | --- | --- |
| A: CLI search | Live GitHub and HN positional queries piped into `jq`; `01a09c86-618c-7251-b9dd-89d39087dcce`, `01a09c86-a4e7-7c91-ab1d-ed930396f214` | Passed |
| B: Unix composition | Strict GitHub plus unconfigured Web returned a valid GitHub record on stdout, diagnostics on stderr, and exit 3; `01a09c87-b103-7d21-8066-74aa20f42ed0` | Passed |
| C: fetch composition | Live GitHub search piped into fetch and `jq`; `01a09c86-a714-7d52-a9ac-f48cf72c7091`. Web search/fetch is covered by fixtures. | GitHub live passed; Web live pending provider key |
| D: MCP | Installed native stdio initialize and tools/list; `01a09cb8-9919-7ba0-a5cc-42fcd2c6fa20`. Deployed MCP and HTTP returned the same HN record; `01a09cba-d748-7051-964a-ddc121fb243b`. | Passed |
| E: Code Mode | Production CLI policy composed fixture search/fetch/follow; `01a09ced-3162-7df0-843f-33f0bf39d019`. Installed CLI completed live HN search/fetch with record `hacker-news:22238335`; `01a09cf0-b982-7833-823b-c603ad59cdb3`. Durable lifecycle, approval, rollback, and prune are proven across separate processes by `cargo xtask native-test`. | Fixture and live integration passed |
| F: external source | Independently built Rust Agent Plugin loaded over real stdio MCP; searched Record targets passed fetch/follow through the loader. Cancelling the caller of a hanging plugin search leaves no plugin process behind. | Integration passed |
| G: self-host | `cargo xtask native-test` ran an actual one-process server, scheduled a fixture watch, and read canonical history JSONL from SQLite. | Passed |
| H: managed | Real Cron, Queue, Worker, and remote D1 after migration 0002 produced `hacker-news:22238335`; history remained isolated from a second tenant. `01a09ce9-f188-76c3-bedf-313fd8ef372e`. | Passed |

The complete local gate passed at `01a09d01-6258-7442-af1c-951c1ee20c87`,
including strict Clippy, tests, cargo-deny, cargo-machete, architecture, complexity,
source conformance, ten required WASM builds, and native/local-Worker runtime tests.
This includes atomic notification batches, migration upgrade/reopen, lease recovery,
retry exhaustion, source metrics, and error-class preservation. SQLite delivery
tests passed 11 cases; D1 shared-SQL tests passed 13 cases. Sender fixtures verified
batch preservation, idempotency headers, and transport-error redaction.
GitHub-hosted CI is a separate acceptance surface. Its first clean Ubuntu run
passed the source checks but exposed a harness deadline that included a cold
Worker release build. The harness now builds before starting Wrangler, so the
60-second deadline measures server readiness. No hosted pass is implied until
the corrected revision completes its own run.

## Managed deployment

The authenticated service is at <https://comsat-cloud.doug-lance.workers.dev>.
Wrangler profile `doug-lance` selected account
`88f9cbf5c4f4e217079bcbf0ca6cb181`. D1 database `comsat` is
`1521142f-6b39-468d-bab0-6879470f17e3`; the Queue is `comsat-watch-runs`.
Cron runs every five minutes.

The live acceptance version was `c95059f7-669b-46c3-8c05-b0470ebbcf61`, deployed
by `01a09cba-2747-7f90-82ef-cbe13e467e5f`. It passed unauthenticated rejection,
disallowed-origin rejection, protocol negotiation (`2025-11-25`), tool discovery,
HTTP and MCP retrieval, fetched identity stability, and tenant isolation.

The subsequent verified build was deployed as
`1f8b0cc2-23ac-4b5c-8ccb-781af9d59274` by
`01a09ce5-a8c2-7803-a0a6-9af148ce7f81`, after D1 migration `0002` applied through
`01a09ce5-9f9d-79c0-bfcc-4d5890da1229`. The installed native binary was rebuilt
with the final Code Mode policy by `01a09ced-7751-7d33-a2ff-432b4964bc4e`.
Post-migration HTTP/MCP identity and tenant checks passed at
`01a09ce7-7099-7eb0-a9e6-bc1190406dd1`.

The final deployed version is `78d2b084-6993-444e-b561-20295c80503c`, deployed by
`01a09cf4-5cf0-7f33-ab76-44d082dae11a`. Postflight
`01a09cf6-d86b-7ce2-b781-5fc158089d8d` verified operator authentication,
unauthenticated and revoked-token rejection, browser-origin rejection, MCP
negotiation and discovery, canonical retained history, and zero active watches.
The temporary acceptance tenant token was revoked and its watch removed. The
operator token remains in the owner's private configuration outside this repository.

Live testing found two defects that compilation and earlier fixtures did not:
native stdio locks held across MCP execution, and an empty cloud origin allowlist
that accepted arbitrary origins. Both have direct regressions and verified fixes.
Malformed cloud HTTP arguments return structured 400 responses. HTTP history now
uses the canonical command graph, including relative `since=30d` filtering.

## Limits that remain explicit

Web search needs a configured provider key before its live acceptance scenario can
pass. GitHub repository search, review-specific traversal, and Discussions are not
implemented. See [source behavior](sources.md) for the exact operation mapping and
bounded result limits.

The managed Stack Exchange egress IP was throttled by the upstream API during
live acceptance. The official error envelope is preserved as `rate_limit`, with
backoff respected. `01a09ce8-6693-74e2-8408-e73943c7df79` verifies that error
contract, not successful cloud retrieval. Native retrieval and fixtures passed.
Webhook delivery is live-proven: one real HTTPS notification carrying two
Hacker News records reached a disposable external receiver with a stable
idempotency key and no credential in the payload, and its delivery row finished
`succeeded` after one attempt. See [notifications](notifications.md).

Native Code Mode uses the existing Incurs executor with a durable SQLite
execution store. `cargo xtask native-test` proves the lifecycle across separate
CLI processes: a completed execution is listed, shown, and read for events from
other invocations; a mutating tool pauses for approval and the pause survives
the process; approval applies the watch; rollback compensates it; and prune
keeps only the newest executions. Reading Code Mode state never creates the
database. The cloud MCP protocol family is limited to what the pinned Incurs
adapter supports.
HTTP/MCP aggregate responses are bounded and collected by that adapter; native
JSONL output streams progressively.

## Deployed v1 build

Worker version `8b1b405e-8cd8-4c9c-98e3-56fd5f692415` carries the v1 work:
corrected MCP transport, the full GitHub object coverage, the shared `serve`
command graph, and D1 migration `0003_codemode_executions.sql`. Apoc receipt
`01a09d79-ebaf-78d2-97d3-ce1a4f450799` probed it directly and passed every
check: MCP initialize and tools/list, camelCase read-only annotations and
object-root output schemas on all nine read-only tools, live MCP and HTTP
search and fetch returning `hacker-news:22238335`, live GitHub repository
search returning only `repository` records, and GitHub Discussions reporting a
structured `authentication` diagnostic with exit code 3 because the managed
deployment has no GitHub token configured.

The managed watch pipeline was re-proven on this build: a watch created through
the authenticated HTTP surface was claimed by Cron, executed through the Queue,
and persisted two Hacker News records to remote D1, readable through
`/history?watch=…`, with the watch rescheduled an hour out. The proof watch was
deleted afterwards and no watch remains active.

## MCP 2025-11-25 tool contracts

The adapter previously serialized annotation hints with snake_case keys such as
`read_only_hint`, and aggregate output schemas had array roots. Both are corrected
upstream in Incurs `8b1a6c4` and in the COMSAT schema caller. Deployed Worker
version `a8b5235f-fef5-4824-a438-5a849f3079be` was probed directly at
`01a09d4b-be83-7392-a7c7-5b3edbea6f87`: every read-only tool advertises camelCase
`readOnlyHint` with no snake_case keys, every advertised `outputSchema` has an
object root with no nested `$schema` and no unresolved local reference, and live
MCP and HTTP search and fetch returned `hacker-news:22238335` with the requested
source filter honored. `cargo xtask cloud-test` asserts the same contract against
a locally built Worker so the gate fails before a deployment can regress it.

Source conformance now validates tool-schema contracts, stable record identity,
structured source-error classes, and active cancellation observed as the pending
source request being dropped. External Agent Plugin process cancellation is
covered separately by `cargo xtask native-test`, which requires a cancelled
search to leave no plugin process behind.

Additional release requirements: source conformance for every plugin; cancellation
and partial failure; strict local quality gate; runtime-neutral WASM checks;
tenant isolation; source throttling; managed endpoint authentication; an open
source deployment that operators can reproduce.

No result is marked complete merely because it compiles or because its configuration
has been written. Missing credentials and unavailable provider coverage remain
explicit limitations until their corresponding runtime evidence exists.
