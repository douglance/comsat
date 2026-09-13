# COMSAT architecture

COMSAT retrieves evidence. Consumers decide what the evidence means. The v1
product specification supplied on September 13, 2026 governs implementation;
the acceptance ledger records which requirements have executable evidence.

```text
[Source plugins: retrieve and normalize]
          | Incurs command definitions
          v
[Incurs ToolCatalog: invocation and cancellation]
          | canonical Record chunks / structured source errors
          v
[COMSAT engine: merge, ordering, deduplication]
          | Record stream
          v
[COMSAT app: shared command graph]
          | capability definitions
          +----------------------+----------------------+
          |                      |                      |
          v                      v                      v
[CLI binding: Unix I/O] [MCP binding: agent I/O] [Code Mode: composition]
```

The native distribution and Worker assemble sources. The engine and app do not
import concrete plugins. Removing first-party sources leaves the core buildable.
Each source is an independent Rust crate and exposes the same Source Profile.

```text
[Watch definition: query + sources + interval]
          | due execution
          v
[Runtime scheduler: native timer / Cron + Queue]
          | query
          v
[Shared retrieval: engine]
          | normalized observations
          v
[Store contract: tenant-scoped persistence]
          | records, run status, first/last observations
          +-------------------------+
          |                         |
          v                         v
[SQLite adapter: self-host] [D1 adapter: managed]
```

## Boundary decisions

* `comsat-types` contains values and validation; it has no Incurs, network client,
  database, or Worker dependency. Timestamp strings use RFC 3339; comparisons use
  parsed instants. Metadata is an object. Record IDs are source-scoped.
* `comsat-source` owns semantic source contracts and conformance. An injected HTTP
  interface is the concrete native/Worker substitution boundary. Source plugins
  construct requests; deployment adapters perform them.
* `comsat-engine` invokes Incurs catalogs. It owns no HTTP or persistence code.
* `comsat-app` defines one command graph and accepts dependencies. `comsat-cli`
  is an additional crate because native process, networking, SQLite, and Code Mode
  dependencies must not enter the shared Worker build.
* Incurs owns plugin packages, MCP, discovery, tool schemas, and Code Mode.
  No protocol or runtime replacement is implemented in COMSAT.
* First-party source text is untrusted tool-result data. It is never evaluated
  as code or interpreted as runtime instructions.

## Deployment and compatibility

Incurs is pinned to an immutable upstream Git revision so its core, Code Mode,
and Cloudflare extension packages share one tested implementation. No sibling
checkout or uncommitted Incurs changes are required. An eventual registry-only
release must preserve that agreement.

The initial Cloudflare MCP adapter serves the lifecycle it implements. A newer
MCP lifecycle requires an upstream Incurs change and protocol tests, rather than
an additional COMSAT protocol path.

The managed deployment must authenticate every remote tool request before
constructing a tenant-scoped app. CORS is an origin constraint, not authentication.
Read commands do not store query results. Watches explicitly authorize storage.

Quality checks must compile shared crates for WASM separately from the native
workspace, where Cargo feature unification enables native runtime features.
