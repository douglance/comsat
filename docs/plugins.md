# Third-party COMSAT sources

A COMSAT source is an Agent Plugin that exposes an MCP tool surface and declares
COMSAT source metadata in `plugin.json`. Incurs owns package loading, MCP
connection, schemas, cancellation, and namespacing. COMSAT only validates the
source profile and turns the connected tools into source records.

## Manifest extension

Declare the source profile under the standard Agent Plugin `extensions` object:

```json
{
  "$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
  "name": "example-source",
  "version": "0.1.0",
  "extensions": {
    "io.comsat.source": {
      "version": 1,
      "id": "example-source",
      "displayName": "Example Source",
      "supportsFetch": true,
      "supportsFollow": false
    }
  }
}
```

`search` is required. `fetch` and `follow` are optional, but if the extension
advertises them the MCP tool must exist. Source IDs are lowercase ASCII words
separated by hyphens.

## Tool contract

The MCP server may expose direct tool names (`search`, `fetch`, `follow`) or
COMSAT-prefixed names (`comsat_example_source_search`). Incurs namespaces tools
under the `mcp.json` server key when the plugin is connected, so COMSAT accepts
the resulting namespaced forms such as `fixture_search`.

`search` accepts a flat COMSAT query object:

```json
{
  "text": "MCP OAuth",
  "limit": 10,
  "since": "2026-01-01T00:00:00Z",
  "until": "2026-02-01T00:00:00Z"
}
```

`fetch` and `follow` accept the canonical tagged COMSAT `Target` object. For a URL target:

```json
{ "type": "url", "source": "example-source", "url": "https://example.com/item/1" }
```

Native source identifiers use `type: "native"` with `source` and `id`; record targets use `type: "record"` with `record`.

Every result must be a canonical COMSAT `Record` or an array/stream of records.
Source-specific fields belong in `metadata`.

## Local loading

Set `COMSAT_PLUGINS` to a platform path-list of Agent Plugin directories before
starting COMSAT:

```bash
COMSAT_PLUGINS="$PWD/examples/external-source" cargo run -p comsat-cli -- source list
```

Plugin runtime data is persistent. Set `COMSAT_DATA_DIR` to choose the root;
otherwise COMSAT uses `$XDG_DATA_HOME/comsat` or `$HOME/.local/share/comsat`.
Each plugin gets a distinct child directory based on its canonical path.

Adding a source does not require changes in `comsat-types`, `comsat-source`,
`comsat-engine`, or `comsat-app` command definitions. The native CLI loads the
package through Incurs, validates `io.comsat.source`, connects the declared MCP
servers, and registers successful sources in the runtime catalog. Invalid
plugins are reported as diagnostics without discarding other successfully loaded
sources.

See `examples/external-source` for a minimal Rust source package. Its source crate pins Incurs to git revision `8b1a6c400b2eb65ee379e099c1d7bd96525f536a` so the example does not depend on a sibling checkout.
