# Local embedding and rerank node balancing

A logical model can opt into equal-weight request rotation among its local provider's configured nodes. This is independent of upstream account/Key assignment. Cloud providers, OAuth credentials, chat, Responses and native session pins keep their existing routing behavior.

Set the model routing policy through the console's model routing editor or `PUT /api/model-routing?model=<logical-name>`:

```json
{
  "channel_ids": [9, 54],
  "local_strategy": "round_robin",
  "retry_times": 0,
  "reprobe_interval_ms": 10000
}
```

The API replaces the model's whole policy: read it first and preserve timeout and other overrides when editing. `channel_ids` is an allowlist, not a preference prefix. For a GPU-only pool list only its GPU source IDs. Cloud backup can remain a separate, compatibility-verified shared fallback target.

`local_strategy` accepts `round_robin` and `ordered`. Omitting it inherits the previous provider Key strategy. `ordered` explicitly retains that previous behavior; it does not introduce a new cross-Key failover policy. Invalid values are rejected. Rotation applies only to embeddings/rerank calls whose provider uses local endpoint mode. It follows the target logical model's policy inside a shared fallback.

Requests rotate within each eligible entrypoint/provider/target-model group. The direct model name, each shared entrypoint, and each owner's private entrypoint have independent cursors. Planning an unused local backup for a successful cloud-primary shared call therefore does not consume the direct model's turns. Private aliases with the same name are isolated by owner and route identity; callers of one shared entrypoint share that entrypoint's rotation. Disabled, unauthorized and cooling sources are excluded by existing checks. A busy node can yield to another available node; when all are busy the request waits within its budget. Retryable upstream/network failures can continue to another local node. Existing explicit 400/429 switch settings still apply; a request rejected with switching disabled stops there. The gateway preserves the request payload, upstream model mapping, billing attribution and full rerank candidates.

The cursor lives in each server process and resets on restart. This is equal request rotation within one process, not a distributed weighted scheduler or a promise of equal work, tokens or GPU utilization. Independent diagnosis aliases remain pinned by their configured source allowlist. No new account permissions or billing discounts are implied.

Before adding a node to a logical model used in vector/rerank shared fallback targets, refresh their compatibility evidence against the final source identities. Removing this feature from an older server requires clearing `local_strategy` from stored policies first, because older releases reject unknown policy fields.
