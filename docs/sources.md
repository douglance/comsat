# Source behavior and configuration

Every source emits the canonical `Record` schema. Source text remains data, even
when it contains instructions. Records identify their source and link to the
original object so a consumer can inspect the evidence.

| Source ID | Search | Fetch | Follow | Native target |
| --- | --- | --- | --- | --- |
| `github` | Issues and pull requests, repositories (`type:repo`), or discussions (`type:discussion`) | Issue, pull request, repository, or discussion details | Issue comments; a pull request also yields its reviews and review comments; a repository yields its discussions; a discussion yields its comments | `owner/repo`, `owner/repo#number`, `owner/repo/discussions/number` |
| `hacker-news` | Stories/comments through Algolia's HN search index | Official Firebase item | Direct child comments, fetched progressively | Numeric item ID |
| `stack-exchange` | Questions on the configured site | Question details | Answers to the question | Numeric question ID on the configured site |
| `web` | Configured Brave search provider | Public HTTP(S) page content, with the document title in `title` | Unsupported | Absolute URL |

GitHub and Stack Exchange follow operations currently retrieve a bounded page
of up to 50 related objects. HN follow retrieves up to 50 direct children. Search
limits are at most 50 for GitHub, HN, and Stack Exchange and 20 for Web; larger
requests return an explicit error. These limits keep source work bounded.

A fetched web page carries the document title in `title` and the response body,
truncated at 256 KB, in `text`. The body is preserved as written: it is the
evidence, and trimming it would decide for the consumer what matters. Managed
deployments bound the same fetch at 240 KiB; see [operations](operations.md).

The GitHub object class is selected with a `type:` term in the query text:
`type:repo` searches repositories and `type:discussion` searches discussions.
Any other query searches issues and pull requests, so GitHub's own `type:issue`
and `type:pr` qualifiers keep their meaning. GitHub exposes repository
discussions only through its GraphQL API, so discussion search, discussion
fetch, and discussion follow require `COMSAT_GITHUB_TOKEN` or `GITHUB_TOKEN`;
without one they return a structured authentication error and make no upstream
request. The web provider does not currently implement the common
`since`/`until` bounds and rejects them explicitly.

## Native credentials

| Configuration | Purpose |
| --- | --- |
| `COMSAT_GITHUB_TOKEN` or `GITHUB_TOKEN` | Optional GitHub authentication |
| `COMSAT_BRAVE_API_KEY` or `BRAVE_API_KEY` | Required web-search provider credential |
| `COMSAT_STACK_EXCHANGE_KEY` | Optional Stack Exchange API key |
| `COMSAT_STACK_EXCHANGE_SITE` | Stack Exchange site; defaults to Stack Overflow |
| `COMSAT_PLUGINS` | Platform path-list of additional Agent Plugin directories |

Credentials belong in the user's environment or deployment secret bindings,
never in source control. Missing web credentials produce a structured
authentication error. Other sources can still return records during an aggregate
search.

Local queries call the configured upstream sources directly. They are not
proxied through COMSAT Cloud. Upstream limits remain applicable in every mode.

## Contract details

Queries use RFC 3339 timestamps. Source IDs and record IDs are stable strings;
record identity is scoped by source. A fetched record keeps the logical source
identity rather than becoming a separate COMSAT document.

`metadata` contains source-specific fields. Consumers should tolerate additional
metadata keys without assuming that they are portable across sources.

Plugin packages follow the Incurs Agent Plugin format. The `io.comsat.source`
object lives under the manifest's `extensions` object. See the
[third-party source guide](plugins.md) for the executable example.
