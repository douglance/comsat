# Watch notifications

Notifications are optional HTTPS webhooks. Without explicit operator configuration,
watches persist observations and send nothing. Local and managed runtimes use the
same delivery contract.

```text
[Watch runner: retrieve evidence] --new records--> [Store: atomic commit]
                                                   |             |
                                              observations   delivery batch
                                                   |             |
                                                   v             v
                                             [History]     [Outbox: retry state]
                                                                 |
                                                           canonical records
                                                                 v
                                                       [HTTPS sender: delivery]
                                                                 |
                                                       JSON + Idempotency-Key
                                                                 v
                                                       [Configured receiver]
```

The store creates one batch for the exact new source/record identities observed by
a watch run. Record updates and the outbox commit in the same SQLite transaction
or D1 batch. Repeated records create no additional notification. No source text
can choose a destination or supply authorization.

## Configuration

For self-hosted operation, explicitly set `COMSAT_WEBHOOK_URL` to a public HTTPS
endpoint before starting `comsat serve`. `COMSAT_WEBHOOK_TOKEN` optionally provides
a bearer token for that endpoint. These apply to the runtime's configured tenant.

For managed operation, add a `TENANT_WEBHOOKS_JSON` secret whose value is a JSON
object mapping authorized tenant IDs to objects with `url` and optional `token`
fields. Keep this JSON in the private Wrangler secrets file, outside source
control. The sender rejects unknown tenants, embedded URL credentials, local
addresses, and non-HTTPS destinations. Redirects are disabled.

The JSON payload has type `comsat.watch.records_observed`, tenant/watch/run IDs,
an observation timestamp, and a `records` array of canonical COMSAT records. It
contains no webhook credential. Receivers must treat record text as untrusted data.

## Delivery and recovery

Delivery uses a stable `Idempotency-Key` header. Receivers should deduplicate that
key: a crash after the receiver accepts a request but before COMSAT records success
can cause another attempt. This is at-least-once delivery, not exactly-once HTTP.

A failed attempt becomes eligible again after 300 seconds. At most three HTTP
attempts are made; exhausted deliveries remain in the `failed` state. Expired
delivery leases are recoverable, and an obsolete lease owner cannot complete a
delivery. The maximum webhook payload is 1 MiB.

Native retries run with the 30-second scheduler. Managed retries run on the
five-minute Cron schedule, at most one pending delivery for each of four rotating
tenants per invocation. Queue execution attempts one pending delivery after a watch
finishes. Watch completion remains successful when its webhook needs a retry.

Changing a configured destination does not silently redirect queued data. A queued
delivery whose destination differs from the current configuration is rejected.
Restore the matching configuration to retry it within the attempt budget.

The `deliveries` table records status, attempts, next-attempt time, and lease state.
Operational logs report status and attempt counts without destination URLs or
credentials. Apply migration `0002_delivery_notifications.sql` before deploying
the managed sender; native installations apply it transactionally once on upgrade.
No live notification receiver was configured during implementation acceptance.
