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

The API replaces the model's whole policy: read it first and preserve timeout and other overrides when editing. `channel_ids` is an allowlist, not a preference prefix. For a GPU-only pool list only its GPU source IDs. Cloud backup can remain a separate shared fallback target. Compatibility diagnosis is optional and never gates saving, normal calls or configured failover.

`local_strategy` accepts `round_robin` and `ordered`. Omitting it inherits the previous provider Key strategy. `ordered` explicitly retains that previous behavior; it does not introduce a new cross-Key failover policy. Invalid values are rejected. Rotation applies only to embeddings/rerank calls whose provider uses local endpoint mode. It follows the target logical model's policy inside a shared fallback.

Requests rotate within each eligible entrypoint/provider/target-model group. The direct model name, each shared entrypoint, and each owner's private entrypoint have independent cursors. Planning an unused local backup for a successful cloud-primary shared call therefore does not consume the direct model's turns. Private aliases with the same name are isolated by owner and route identity; callers of one shared entrypoint share that entrypoint's rotation. Disabled, unauthorized and cooling sources are excluded by existing checks. A busy node can yield to another available node; when all are busy the request waits within its budget. Retryable upstream/network failures can continue to another local node. Existing explicit 400/429 switch settings still apply; a request rejected with switching disabled stops there. The gateway preserves the request payload, upstream model mapping, billing attribution and full rerank candidates.

The cursor lives in each server process and resets on restart. This is equal request rotation within one process, not a distributed weighted scheduler or a promise of equal work, tokens or GPU utilization. Independent diagnosis aliases remain pinned by their configured source allowlist. No new account permissions or billing discounts are implied.

Adding a node changes the source set. You may run compatibility diagnosis against the final identities when troubleshooting numerical differences; missing, stale or failed evidence does not block the new node or ordinary fallback. Removing this feature from an older server requires clearing `local_strategy` from stored policies first, because older releases reject unknown policy fields.

## Configure a GPU pool with cloud fallback

1. Add a local provider in “供应商与接入”, choose the multiple-API-address mode, and add one credential/anonymous entry per service URL. Configure the same logical model at each address, with its own upstream name where necessary. Keep existing source identities when editing.
2. Read the logical model's current routing policy, then save it with only the intended GPU source IDs in `channel_ids` and `local_strategy: "round_robin"`. These IDs identify the underlying sources, not the provider or credential record. Preserve any current timeout and retry overrides: the PUT replaces the whole policy.
3. To add cloud backup, create a shared or private virtual entrypoint with a local-provider/model target and a cloud-provider/model target in the desired order. Reverse the targets for cloud-first operation. Each target keeps its own provider's Key policy; the local target uses the target logical model's node rotation.
4. Authorize the entrypoint and all required source IDs in the caller's channel allowlist. Shared entries may delegate internal-only logical models; private entries cannot. Send a request with the entrypoint name and inspect the request's actual attempts, rather than assuming equal GPU utilization from equal request turns.

For longer vector requests, an administrator can open “系统设置 → 请求等待时间 → 编辑等待时间” and adjust the global non-streaming completion timeout and total request budget. The former bounds a non-streaming attempt; the latter includes waiting, retries and fallback. The UI accepts 1–3,600 seconds per field and stores milliseconds. These changes affect all calls that inherit the global values. Shared and private virtual entrypoints normally inherit these settings: the current model-routing editor and PUT API only accept configured real logical models, so they cannot save a timeout override under a virtual name. A target logical model's node rotation still applies, but its timeout override does not automatically become the virtual entrypoint's timeout. The client/proxy timeout must accommodate the total wait. Neither retry count zero nor missing compatibility evidence disables configured vector fallback.

Billing remains per actual source/model and reported usage. Rotation neither merges upstream prices nor turns unknown usage into known zero. See [Key pools and virtual strategies](../provider-key-pools.md), [shared models](../shared-virtual-models.md) and [vector fallback](../vector-fallback.md).
