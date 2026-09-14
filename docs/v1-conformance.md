Artifact: V&V conformance audit V1-CONFORMANCE-2026-09-13
Subject: COMSAT v1 product requirements and deployed implementation
Status: in review
Version: 1.0
Owner: quality engineer
Inputs: COMSAT product specification sections 1-79, repository commit 2a43f85e24fcbdbd3fe0850cba8d2bd26a84e2e7, deployed Worker 78d2b084-6993-444e-b561-20295c80503c, GitHub Actions run 34788597060
Revision: 2026-09-13 follow-up. Findings F1, F3, F4, F5, F7, and F8 are closed with the evidence recorded under each. F2 and F6 remain open and both are blocked on a provider credential the operator must supply.
Governing references: COMSAT PRD v1.0, MCP schema reference 2025-11-25 at https://modelcontextprotocol.io/specification/2025-11-25/schema, repository docs and source cited below
Scope: This artifact records factual conformance evidence and gaps. It is not an approval request.

# COMSAT V1 Conformance Audit

## Quality Gate

Verdict: FAIL for full v1 release. Every finding that engineering can close is closed; the two that remain need provider credentials.

Phase: quality and validation - partial.

Verification status: PASS for the committed 0.1 build evidence at 2a43f85. The local gate is recorded as passed in `docs/acceptance.md:18-24`, and GitHub Actions run 34788597060 shows status Success for commit 2a43f85. The current shared worktree has an additional root-owned MCP annotation check that has not passed yet.

Validation status: FAIL for full v1. COMSAT has a strong 0.1 implementation, but several v1 requirements are incomplete, contradicted, or unverified.

## Release-Blocking Findings

F1 - CLOSED - MCP 2025-11-25 tool contracts are satisfied. Aggregate output schemas now have object roots and the hosted adapter emits camelCase annotation hints, fixed upstream in Incurs `8b1a6c4` and in the COMSAT schema caller. `xtask/src/cloud_test.rs` asserts the contract against a locally built Worker in every gate run, and deployed Worker `a8b5235f-fef5-4824-a438-5a849f3079be` was probed directly at Apoc receipt `01a09d4b-be83-7392-a7c7-5b3edbea6f87`: every read-only tool advertises `readOnlyHint` with no snake_case keys, every advertised `outputSchema` has an object root with no nested `$schema` and no unresolved local reference, and live MCP and HTTP search and fetch returned `hacker-news:22238335`.

F2 - OPEN, credential-blocked - live web search is still unproven. The web source requires a Brave Search provider key in `COMSAT_BRAVE_API_KEY` (native) or `BRAVE_SEARCH_API_KEY` (managed); no key exists on the build machine or in the Worker secrets. Obtaining one requires creating a Brave Search API account, which is the operator's to create. Everything else about the source is fixture-tested, and a missing key produces a structured authentication error while other sources still return records.

F3 - CLOSED - GitHub coverage includes repositories, repository discussions, and pull-request review discussions. Search selects the object class with `type:repo` and `type:discussion`; fetch resolves repository, issue, pull-request, and discussion targets; follow returns issue comments, a pull request's reviews and review comments, a repository's discussions, and a discussion's comments. Discussions use GitHub's GraphQL API and require a token, which the source reports as a structured authentication error before making any request. Twelve plugin tests cover the paths, and every operation was verified against live GitHub.

F4 - CLOSED - Source Profile conformance is deep enough for the v1 suite. `crates/comsat-source/src/conformance/schema.rs` walks tool schemas as a contract consistency check over known schema keywords, so data positions are left alone while schema arrays, embedded resources, anchors, and local references are validated. The suite also checks stable record identity, streaming record deserialization, structured `SourceError` classes, and cancellation proven by observing the pending source request being dropped rather than by trusting the reported outcome. External Agent Plugin process cancellation is covered by `cargo xtask native-test`, which hangs a search inside the fixture plugin, kills the caller, and requires the plugin process to exit.

F5 - CLOSED - the durable Code Mode lifecycle is exposed. Executions, oversized artifacts, and snippets persist in the same SQLite database as watches and history, and `comsat code` exposes list, show, events, artifact, approve, reject, resume, cancel, rollback, and prune. Rollback compensates `watch_add` by deleting the watch it created and reports any other action as uncompensated rather than claiming a revert it cannot perform. `cargo xtask native-test` proves the lifecycle across separate CLI processes, including that reading Code Mode state never creates the database.

F6 - OPEN, credential-blocked - managed Stack Exchange retrieval is still throttled. Re-probed on the deployed Worker after this work: Stack Exchange answers `throttle_violation: too many requests from this IP, more requests available in 35237 seconds` for the shared Cloudflare egress address, across repeated attempts. The error contract is correct and preserved as `rate_limit`; successful managed retrieval needs a Stack Apps API key in `STACK_EXCHANGE_API_KEY`, which raises the quota from the shared per-IP allowance to a per-key allowance. Registering that key requires a Stack Apps account and is the operator's to create.

F7 - CLOSED - `serve` is owned by the shared `comsat-app` command graph behind a `serve` feature, with native transport logging and notification flushing attached through `ServeHooks`. The HTTP surface it exposes is the same graph every other surface uses. Registering its options also restored the documented `--addr` flag, which the previous command silently rejected.

F8 - CLOSED - notification delivery is live-proven. A self-hosted watch over Hacker News observed two records and delivered one HTTPS POST to a disposable external receiver carrying type `comsat.watch.records_observed`, the tenant and watch identity, both canonical records, and a stable `Idempotency-Key`, with no credential in the payload. The delivery row finished `succeeded` after one attempt and the receiver was deleted afterwards.

## V&V Evidence

Follow-up verification evidence (2026-09-13, after the findings above were addressed):

- Deployed Worker `a8b5235f-fef5-4824-a438-5a849f3079be` passed a direct MCP contract and live-retrieval probe: `01a09d4b-be83-7392-a7c7-5b3edbea6f87`.
- `cargo xtask check` passed on the corrected tree, including `cloud-test` against a locally built Worker and `native-test` covering the durable Code Mode lifecycle and external plugin cancellation.
- Every new check was mutation-probed: removing local-reference resolution, leaking a cancelled source request, dropping pull-request review traversal, removing the Discussions token guard, and removing rollback compensation each turn the relevant check red.
- Remaining open findings are F2 and F6; both require a provider credential and neither is reachable from this repository.

Verification evidence:

- The audited base was `main` at `2a43f85e24fcbdbd3fe0850cba8d2bd26a84e2e7`. During this audit, the shared worktree gained a root-owned `xtask/src/cloud_test.rs` MCP annotation check and this quality-owned document.
- GitHub Actions run 34788597060 shows status Success for commit `2a43f85`.
- `docs/acceptance.md:18-24` records full local `cargo xtask check` pass receipt `01a09d01-6258-7442-af1c-951c1ee20c87`.
- `xtask/src/checks.rs:13-50` includes fmt, strict Clippy, tests, cargo-deny, cargo-machete, architecture, complexity, conformance, WASM, native-test, and cloud-test.
- Native MCP stdio probe receipts:
  - `01a09d11-f4e3-7930-a918-00f013224995`: tools/list exposes facade tools with camelCase annotations and no advertised output schemas.
  - `01a09d12-41a7-79e0-b3f2-5d3d1b3d99c4`: `get_tool_details` returns object-shaped `structuredContent` but includes internal `comsat_search.outputSchema` with an array root.
- Cloudflare MCP annotation regression is now represented by `xtask/src/cloud_test.rs:90-105`, which checks hosted `tools/list` for protocol `readOnlyHint` keys.

Validation evidence:

- Current docs say 0.1 is implemented and deployed, but do not certify every v1 requirement complete: `docs/acceptance.md:3-5`.
- Acceptance scenarios A, B, D, E, F, G, and H have evidence in `docs/acceptance.md:9-16`.
- Scenario C is partial because web live acceptance still needs a provider key: `docs/acceptance.md:11`.
- Managed deployment is documented at `docs/acceptance.md:31-58`, including Worker, D1, Queue, Cron, final version, and postflight checks.

## Quality Attributes Checked

Security: partial pass. Managed endpoints require bearer authentication, and tenant identity is not accepted from public tool arguments in `docs/operations.md:78-99`. Browser origin rejection is verified in `docs/acceptance.md:52-56`. Source text is treated as untrusted data in `docs/architecture.md:61-62`. Open risk remains MCP protocol conformance.

Reliability: partial pass. Engine fan-out uses bounded source concurrency and buffers in `crates/comsat-engine/src/merge.rs:35-52` and `crates/comsat-engine/src/merge.rs:162-180`. Watches fence writes by tenant, run, and lease in `docs/operations.md:101-108`. Open risk remains incomplete conformance checks for cancellation and live provider variance.

Performance and resource bounds: partial pass. Cloud retrieval limits are documented at `docs/operations.md:101-108`, and complexity budgets are enforced by `xtask/src/complexity.rs:16-23`. No load or latency acceptance target has been proven for production.

Maintainability: partial pass. Dependency boundaries are executable through `xtask/src/checks.rs:43-50`, `xtask/src/architecture.rs`, and the workspace pins Incurs at one Git revision in `Cargo.toml:24-27`. The main maintainability risk is carrying MCP wrapping behavior partly upstream and partly in COMSAT.

Usability and Unix composition: partial pass. JSONL and stderr contracts are documented in `docs/operations.md:13-15`, and native tests cover stdin target composition in `crates/comsat-cli/tests/native.rs`. Full usability remains blocked where missing credentials produce partial source behavior.

## Requirement Matrix

Status values:

- Proven: inspected source plus executable test, runtime, or production evidence satisfies the requirement.
- Partial: materially implemented, but a required part is missing or only fixture-tested.
- Incomplete: known missing implementation or runtime proof.
- Unverified: no inspected evidence was found in this audit.
- Contradicted: inspected evidence conflicts with the requirement.
- N/A: requirement defines language or context, not an implementation obligation.

## Section 3 Product Goal Detail

| Goal | Status | Evidence and gap |
| --- | --- | --- |
| 3.1 Fast native CLI | Partial | Native CLI exists and is tested. Local dispatch is now measured at a 10-11 ms median per command in `docs/acceptance.md`; no throughput target under load has been set. |
| 3.2 MCP access for agents | Partial | Native and hosted MCP are present. 2025-11-25 tool contract gaps remain in F1. |
| 3.3 Incurs Code Mode composition | Partial | Code Mode search/fetch/follow works in fixtures and live HN. Durable lifecycle/history is missing in F5. |
| 3.4 Normalize heterogeneous results into one schema | Proven | `Record` is canonical in `crates/comsat-types/src/record.rs:7-20`, and source conformance validates fixture records. |
| 3.5 Third parties can add sources without COMSAT core changes | Partial | External source example exists, but source conformance is incomplete. |
| 3.6 Use Incurs instead of a parallel runtime framework | Partial | Incurs owns command/MCP/Code Mode boundaries. The current MCP fix still belongs upstream, so this remains not fully validated. |
| 3.7 First-party and third-party sources use the same Source Profile | Partial | Source Profile is shared, but section 66 conformance coverage is incomplete. |
| 3.8 Unix tool composition | Proven | JSONL, stdin targets, and partial-failure stdout behavior have tests and acceptance evidence. |
| 3.9 Rust implementation except Code Mode programs | Partial | Workspace production crates are Rust. This audit did not run a repository-wide language/source audit. |
| 3.10 Fully open source | Partial | Repository is MIT and public, but this audit did not verify every deployment artifact or generated file. |
| 3.11 Fully local use without COMSAT account | Partial | Local retrieval works without account per docs and acceptance, but full provider coverage still needs user credentials where upstreams require them. |
| 3.12 Free self-hosting | Partial | One-process SQLite self-host is implemented and tested. Operational self-host packaging was not fully audited. |
| 3.13 Managed cloud service | Partial | Cloudflare Worker, D1, Queue, Cron deployment and managed watch proof are recorded. Full v1 managed coverage remains partial. |
| 3.14 Core model independent of deployment environment | Partial | Shared domain crates and store contracts exist. Complete self-host/managed equivalence is not fully proven. |
| 3.15 Strict quality, complexity, dependency, architecture rules | Partial | Rules exist and passed at commit 2a43f85. Current shared worktree has a new red MCP cloud test. |

## Command Surface Detail

| Command requirement | Status | Evidence and gap |
| --- | --- | --- |
| `comsat search` | Proven | Built in `crates/comsat-app/src/commands/mod.rs:31` and live-tested. |
| `comsat fetch` | Proven | Built in `crates/comsat-app/src/commands/mod.rs:32` and live-tested through GitHub. |
| `comsat follow` | Proven | Built in `crates/comsat-app/src/commands/mod.rs:33` and fixture-tested. |
| `comsat source list` | Proven | Built in `crates/comsat-app/src/commands/mod.rs:39-67`. |
| `comsat source inspect` | Proven | Built in `crates/comsat-app/src/commands/mod.rs:70-101`. |
| `comsat source doctor` | Proven | Built in `crates/comsat-app/src/commands/mod.rs:104-119`. |
| `comsat source test` | Partial | Built in `crates/comsat-app/src/commands/mod.rs:132-159`, but it checks registration consistency, not full section 66 conformance. |
| `comsat watch add/list/delete` | Proven | Built in `crates/comsat-app/src/commands/mod.rs:48-53` and `180-232`; self-host and managed watch acceptance exists. |
| `comsat history` | Proven | Built in `crates/comsat-app/src/commands/mod.rs:235-316`; acceptance covers canonical retained history. |
| `comsat serve` | Partial | Built in native CLI at `crates/comsat-cli/src/main.rs:57-61`, not owned by `comsat-app` as section 53 requires. |

## Section 66 Source Conformance Detail

| Section 66 check | Status | Evidence and gap |
| --- | --- | --- |
| Agent Plugin package loads through Incurs | Partial | First-party plugin tests call `load_agent_plugin`, for example `plugins/github/tests/conformance.rs:11-21`. This audit did not prove arbitrary third-party package loading through the same conformance path. |
| `io.comsat.source` extension is valid | Partial | `xtask/src/conformance.rs:103-122` checks version, id presence, and display name. It does not validate SourceId syntax. |
| SourceId is valid | Incomplete | The manifest check accepts any nonempty id at `xtask/src/conformance.rs:110-118`. |
| `search` capability exists | Proven | `crates/comsat-source/src/conformance.rs:53-59` requires advertised search and tool presence. |
| Search input schema conforms | Partial | `crates/comsat-source/src/conformance.rs:95-97` only checks that schema is present and non-null. |
| Search output conforms to `Record` | Partial | Fixture results deserialize to `Vec<Record>` and call `Record::validate`; this does not prove every runtime stream item shape. |
| Stream records deserialize | Partial | Conformance consumes collected ToolCatalog data in `crates/comsat-source/src/conformance.rs:187-203`, not live streaming transport chunks. |
| Record URLs are valid | Partial | `Record::validate` calls URL validation in `crates/comsat-types/src/record.rs:37-49`, and fixtures pass. Live streams were not exhaustively checked. |
| Record IDs are stable | Partial | `crates/comsat-source/src/conformance.rs:59-66` compares repeated fixture search record IDs. Live sources were not exhaustively checked. |
| Structured errors conform | Incomplete | Current conformance treats tool errors as strings in `crates/comsat-source/src/conformance.rs:201-216`. |
| Cancellation propagates | Incomplete | No inspected conformance test exercises cancellation through ToolCatalog, source runtime, and upstream request. |
| `fetch` conforms when advertised | Partial | Fixture target operations call and validate `Record` at `crates/comsat-source/src/conformance.rs:133-151`. |
| `follow` conforms when advertised | Partial | Fixture target operations validate returned records at `crates/comsat-source/src/conformance.rs:153-172`; cancellation and streaming semantics remain untested. |

| Section | Status | Evidence and gap |
| --- | --- | --- |
| 1 Purpose | Partial | Architecture and README preserve evidence-first retrieval. Full v1 validation fails because several source and MCP contracts remain open. |
| 2 Product Definition | Partial | Rust/Incurs domain layer is documented in README and `docs/architecture.md:47-60`. Hosted and plugin surfaces are present, but full MCP 2025-11-25 conformance fails. |
| 3 Product Goals | Partial | See the granular section 3 matrix above. |
| 4 Non-Goals | Partial | No inspected core schema or docs add CRM, sentiment, lead, or sales concepts, but this audit did not run a repository-wide forbidden-concept check. |
| 5 Normative Language | N/A | Language definition only. |
| 6 Relationship to Incurs | Partial | `docs/architecture.md:59-60` says Incurs owns MCP/plugin/Code Mode. MCP adapter behavior still needs upstream fix for 2025-11-25 object schemas. |
| 7 Incurs Conceptual Mapping | Partial | Commands derive from Incurs definitions. `serve` is native CLI-only rather than app-owned. |
| 8 Fundamental Information Model | Proven | `Record`, `Query`, `Target`, and `SourceId` exist in `crates/comsat-types`. |
| 8.1 SourceId | Partial | Source behavior uses lowercase IDs in `docs/sources.md:7-12`; this audit did not cite the exact `SourceId` validator implementation. |
| 9 Canonical Record | Proven | `crates/comsat-types/src/record.rs:7-20` defines the small canonical schema and metadata object. |
| 9.1 Requirements | Partial | URL and metadata validation are implemented in `crates/comsat-types/src/record.rs:22-50`; source fabrication is not directly conformance-tested across all plugins. |
| 10 Query | Proven | `crates/comsat-types/src/query.rs:7-13` matches the required common query shape. |
| 11 Target | Proven | `crates/comsat-types/src/target.rs:6-12` supports record, URL, and native identifiers. |
| 12 Source Profile | Partial | `search`/`fetch`/`follow` profile exists; conformance is too shallow for all section 66 checks. |
| 13 search | Partial | Search is implemented and fixture/live-tested. Progressive MCP streaming and cancellation conformance are not fully proven. |
| 14 fetch | Partial | Fetch is implemented for first-party sources where meaningful; web live fetch is blocked by search provider credentials. |
| 15 follow | Partial | Follow exists for GitHub issue comments, HN children, and Stack Exchange answers. Web follow is unsupported by design. GitHub review/discussion traversal is missing. |
| 16 Streaming | Partial | Native JSONL streams progressively per `docs/acceptance.md:87-88`; HTTP/MCP aggregates are bounded and collected. Cancellation conformance is incomplete. |
| 17 Source Plugin Model | Partial | Agent Plugin packaging is used; third-party example works. `source test` does not yet run full semantic conformance. |
| 18 Source Identification | Proven | Plugin extension metadata is documented in `docs/plugins.md:8-20` and checked in `xtask/src/conformance.rs:103-122`. |
| 19 First-Party Sources | Partial | Four independent plugin crates exist. GitHub repository/discussion/review coverage is missing, and web live use needs credentials. |
| 20 Third-Party Sources | Partial | External example exists in `examples/external-source`, and docs say no core changes are required. The automated acceptance suite is not exhaustive. |
| 21 Runtime Binding | Partial | Native and Worker assemble equivalent source contracts. Managed cloud is not proven for all first-party sources because web key and Stack Exchange throttle remain. |
| 22 COMSAT Engine | Partial | Inspected engine files own merge/dedupe behavior. This audit did not exhaustively prove absence of all forbidden ownership across the crate. |
| 23 Aggregate Search | Proven | Partial failure semantics are tested in `crates/comsat-app/src/tests.rs:144-216`. |
| 24 Deduplication | Proven | `crates/comsat-engine/src/dedupe.rs:22-33` dedupes by source ID, URL, and native metadata IDs. |
| 25 CLI | Partial | Required commands exist, but `serve` is added in `comsat-cli`, not `comsat-app`: `crates/comsat-app/src/commands/mod.rs:27-37`, `crates/comsat-cli/src/main.rs:57-61`. |
| 26 CLI Examples | Partial | Search, fetch, jq, and JSONL have evidence. Web example is blocked live by provider key. |
| 27 Unix I/O Contract | Proven | JSONL/stdout/stderr/exit behavior is documented in `docs/operations.md:13-15` and covered by native tests. |
| 27.1 stdout | Partial | Native output tests were inspected indirectly; this audit did not cite every stdout/stderr assertion. |
| 27.2 stderr | Proven | `docs/operations.md:13-15` defines diagnostics separately. |
| 27.3 stdin | Proven | `crates/comsat-cli/tests/native.rs` covers stdin record targets into fetch and follow. |
| 27.4 JSONL | Proven | Search/fetch/follow/history use JSONL per `docs/operations.md:13`. |
| 27.5 Human output | Partial | Human help is preserved in output tests, but human rendering is minimal. |
| 27.6 Side effects | Proven | Read commands do not persist; `docs/operations.md:43-45` says reading sources does not create the database. |
| 28 Exit Semantics | Proven | Partial failure and invalid invocation exits are tested and documented. |
| 29 MCP Interface | Partial | MCP exists and is deployed. 2025-11-25 object-root output schema conformance fails for internal aggregate tool schemas. |
| 30 Agent Philosophy | Proven | Source text remains data in `docs/sources.md:3-5` and `docs/architecture.md:61-62`. |
| 31 Incurs Code Mode | Partial | Code Mode fixture and live HN composition passed, but durable lifecycle/history is not exposed. |
| 32 Remote Execution | Partial | No competing COMSAT remote protocol was found in inspected files; this audit did not exhaustively scan all protocol code. |
| 33 Local Mode | Partial | README states local queries require no COMSAT account, and source docs say local queries are not proxied. This is doc-backed plus acceptance-backed for retrieval, not exhaustive. |
| 34 Self-Hosted Mode | Partial | SQLite one-process scheduler is implemented and tested. Full MCP/HTTP self-host persistence equivalence is not fully acceptance-tested for all sources. |
| 35 Managed COMSAT Cloud | Partial | Worker/D1/Queue/Cron are deployed. Commercial operations such as backups and notifications are not live-proven. |
| 36 Cloud Runtime | Proven | Wrangler config uses Worker, D1, Queue, and Cron in `crates/comsat-cloud/wrangler.jsonc:16-40`. |
| 37 Incurs Cloudflare Integration | Partial | Pinned Incurs adapter is used; modern MCP schema conformance requires upstream work. |
| 38 Cloudflare Runtime Constraints | Partial | WASM builds and cloud limits are enforced. No production load proof was inspected for the six-connection constraint. |
| 39 Persistence Model | Partial | Migrations were listed but not line-cited in this audit. |
| 40 Record Persistence | Partial | Store contracts and migrations appear to persist required record fields, but this audit did not cite exact schema lines. |
| 41 Watches | Partial | Watches persist and run. Notifications are implemented but not live-configured. |
| 42 History | Proven | History returns canonical records in inspected command code at `crates/comsat-app/src/commands/mod.rs:235-316`, and acceptance says HTTP history uses the command graph. |
| 43 Self-Hosted and Cloud Equivalence | Partial | Shared domain logic is present. Provider-specific live gaps prevent full equivalence proof. |
| 44 Source: GitHub | Partial | Official REST issue/PR implementation exists. Repository search, discussions, and review traversal are missing. |
| 45 Source: Hacker News | Proven | HN search/fetch/follow have fixture coverage and live acceptance evidence in `docs/acceptance.md:9,13,16`. |
| 46 Source: Stack Exchange | Partial | Native and fixtures passed; managed cloud success is throttled and unproven. Backoff handling is documented as preserved. |
| 47 Source: Web Search | Partial | Vendor-independent core contract is preserved, but live web search needs a Brave key. |
| 48 Source Configuration | Proven | Source credentials are environment or secret bindings in `docs/sources.md:23-39` and `docs/operations.md:83-94`. |
| 49 Errors | Partial | Structured source error classes are encoded in `crates/comsat-app/src/commands/schema.rs:51-89`; source-by-source mapping coverage is not fully audited. |
| 50 Partial Failure | Proven | Tests cover strict, nonstrict partial, and total source failure. |
| 51 Cancellation | Partial | Engine has cancellation tokens in `crates/comsat-engine/src/merge.rs:120-128` and `203-208`; conformance tests do not prove propagation through plugins and upstream requests. |
| 52 Repository Structure | Partial | Workspace is compact and close to requested structure, with `comsat-cli` added for native runtime dependencies. |
| 53 Crate Responsibilities | Partial | Most boundaries match. `serve` lives in `comsat-cli`, not `comsat-app`. |
| 54 Plugin Structure | Partial | Plugins are independent crates, but layouts are flatter than the example. This is acceptable unless complexity grows. |
| 55 Modularity Policy | Partial | Crate boundaries are justified in `docs/architecture.md:47-58`; this is architecture rationale, not full proof. |
| 56 Dependency Direction | Proven | Executable architecture checks enforce dependency boundaries in `xtask/src/architecture.rs`. |
| 57 Rust Requirements | Proven | Edition/MSRV and unsafe forbid are in `Cargo.toml:6-10` and `Cargo.toml:45-47`. |
| 58 Linting | Proven | Strict Clippy policy exists in `Cargo.toml:48-55` and `xtask/src/checks.rs:16-28`. |
| 59 Complexity Limits | Proven | Limits are encoded in `xtask/src/complexity.rs:16-23`. |
| 60 Function Design | Partial | Complexity checks enforce structure. This audit did not manually inspect every production function for abstraction purity. |
| 61 File Design | Partial | File hard limits are enforced. Some plugin crates use flat files, but they remain under current thresholds. |
| 62 Trait Policy | Partial | Traits exist for real boundaries such as store, HTTP client, and source runtime. This audit did not prove every trait has current substitution value. |
| 63 Dependency Policy | Proven | `cargo deny` and `cargo-machete` are in the gate at `xtask/src/checks.rs:29-39`. |
| 64 Architecture Validation | Proven | Architecture validation is executable in `xtask/src/architecture.rs` and included in the gate at `xtask/src/checks.rs:43-50`. |
| 65 WASM Validation | Proven | WASM package checks are encoded in `xtask/src/wasm.rs:7-38`. |
| 66 Source Conformance Suite | Partial | Current suite misses several required semantic checks, including cancellation, structured error conformance, and stream deserialization through plugin transports. |
| 67 Testing Requirements | Partial | Unit, fixture, contract, integration, self-host, cloud, and Code Mode tests exist. Live provider and protocol-edge coverage remain incomplete. |
| 68 CI Quality Gate | Partial | `cargo xtask check` implements required categories and hosted CI passed for commit 2a43f85, but the shared worktree now contains a root-owned red MCP annotation test that has not passed yet. |
| 69 Observability | Partial | Source and watch metrics are documented in `docs/operations.md:125-137`. Physical D1 size, Worker failures, and Queue backlog depend on Cloudflare platform metrics and were not audited here. |
| 70 Security | Partial | Auth, tenant isolation, origin checks, secret handling, and untrusted content are addressed. MCP schema conformance remains a security-adjacent interoperability risk. |
| 71 Untrusted Content | Proven | Explicitly documented in `docs/sources.md:3-5` and `docs/architecture.md:61-62`. |
| 72 Open-Source Requirement | Partial | README and operations docs state MIT licensing for code and deployment adapters. This audit did not verify every generated/deployment artifact license. |
| 73 Deployment Modes | Partial | Local, self-hosted, and managed modes exist. Full provider equivalence is not proven. |
| 74 Implementation Phases | Partial | Phase 1 is partially complete; Phase 2 and Phase 3 have substantial implementation; Phase 4 has deployed managed runtime but not full managed v1 coverage. |
| 75 Required Acceptance Scenarios | Partial | A, B, D, E, F, G, H have evidence. C is partial because live web search/fetch is pending provider key. |
| 76 Architectural Acceptance Tests | Partial | Most invariants are enforced. MCP protocol conformance and same-graph ownership of `serve` remain gaps. |
| 77 Design Litmus Test | Proven | No inspected core feature crosses into high-level business interpretation. |
| 78 Fundamental System Model | Partial | System model exists. Full production proof for all sources and MCP 2025-11-25 remains incomplete. |
| 79 Governing Principle | Partial | Thin domain-over-Incurs design is preserved, but Incurs MCP adapter gaps currently block full conformance. |

## Acceptance Scenario Matrix

| Scenario | Status | Evidence and gap |
| --- | --- | --- |
| A - direct CLI search | Proven | Live GitHub and HN search receipts in `docs/acceptance.md:9`. |
| B - Unix composition | Proven | Valid stdout diagnostics/stderr and exit 3 receipt in `docs/acceptance.md:10`. |
| C - fetch composition | Partial | Live GitHub fetch passed. Required web search/fetch remains credential-gated in `docs/acceptance.md:11`. |
| D - agent search | Partial | Native and deployed MCP discovery/retrieval passed, but MCP 2025-11-25 output schema conformance is unresolved. |
| E - Code Mode composition | Partial | Fixture and live HN integration passed. Durable lifecycle/history is not exposed. |
| F - third-party source | Partial | External Rust Agent Plugin integration passed. The conformance suite is incomplete. |
| G - self-hosted watch | Proven | Native test ran one-process scheduler and SQLite history per `docs/acceptance.md:15`. |
| H - managed watch | Proven | Cron/Queue/D1 watch proof exists in `docs/acceptance.md:16`. Managed provider coverage remains tracked separately under sections 19, 21, 43, and 46. |

## Architectural Invariant Matrix

| Invariant | Status | Evidence and gap |
| --- | --- | --- |
| 1 Removing first-party source crates leaves source-less core compilable | Unverified | Architecture checks forbid some dependency edges, but this audit did not actually remove source crates and compile the core. |
| 2 Adding a new source requires no core-engine modification | Partial | External example proves the path; full semantic conformance still shallow. |
| 3 COMSAT implements no independent MCP protocol | Proven | Incurs owns MCP per `docs/architecture.md:59-60`. |
| 4 COMSAT implements no independent plugin packaging format | Proven | Agent Plugin extension is used in `docs/plugins.md:8-20`. |
| 5 COMSAT implements no independent Code Mode runtime | Proven | Existing Incurs local executor is used. |
| 6 CLI and MCP derive from same Incurs command definitions | Partial | Retrieval commands do; native `serve` is CLI-only. |
| 7 Source records share one canonical schema | Proven | `Record` schema is central and conformance validates fixture records. |
| 8 First-party and third-party sources pass same Source Profile conformance | Partial | Existing suite applies to first-party fixtures and external example, but required semantic checks are missing. |
| 9 Cloud dependencies do not appear in runtime-neutral crates | Proven | Architecture check enforces forbidden dependencies. |
| 10 Self-host and managed cloud share domain behavior | Partial | Store/runtime adapters share contracts, but live provider equivalence is incomplete. |
| 11 Unix composition does not require COMSAT-specific downstream commands | Proven | JSONL stdout and jq examples have evidence. |
| 12 High-level business interpretation remains outside core | Proven | No inspected core types encode those concepts. |

## Evidence Receipts

- Native MCP tools/list probe: `01a09d11-f4e3-7930-a918-00f013224995`.
- Native MCP internal tool details probe: `01a09d12-41a7-79e0-b3f2-5d3d1b3d99c4`.
- DevSQL context gather: `01a09d12-8d66-7f42-9922-eb4d27795ec2`.
- Local gate receipt recorded by the repo: `01a09d01-6258-7442-af1c-951c1ee20c87`.
- GitHub Actions run: `34788597060`.
