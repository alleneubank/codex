# Thread removal

`thread/archive` and `thread/delete` reject attempts to remove a live internal
worker with JSON-RPC error `-32600`. The worker's owner controls its shutdown.
For example, a Guardian reviewer remains available to its parent conversation
after a client tries to archive or delete it.

After the owner releases the worker, its saved conversation can be archived or
deleted normally. Ordinary client-controlled threads keep their existing behavior.

## User-attention lifecycle (experimental)

Clients that expose a synchronous decision after a completed turn can bracket that UI with a correlated lifecycle. Initialize with `capabilities.experimentalApi = true`, then call `thread/userAttention/start` before displaying the decision:

```json
{
  "method": "thread/userAttention/start",
  "id": 27,
  "params": {
    "threadId": "thr_123",
    "turnId": "turn_123",
    "attentionId": "plan-prompt-123",
    "kind": "planImplementation"
  }
}
```

`turnId` must identify the latest completed turn whose post-turn hook context remains available. `attentionId` is an opaque client-generated correlation value; it is never included in notification-hook input. Lifecycles are isolated by connection, thread, and attention ID, so different connections or threads may use the same value. A duplicate active start for the same key is rejected.

Complete the lifecycle for every selection, dismissal, and cancellation path:

```json
{
  "method": "thread/userAttention/complete",
  "id": 28,
  "params": {
    "threadId": "thr_123",
    "attentionId": "plan-prompt-123"
  }
}
```

Both methods return `{}`. Completion is safe to retry. Closing the owning connection or tearing down the thread completes any remaining owned lifecycle exactly once. Hook failures do not suppress the client decision UI; clients should surface signaling failures separately and continue to present the decision.
