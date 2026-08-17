# Pantheon Architecture Reading Recipes

Conditional retrieval guidance for common kinds of change. This is not a
specification and not an inventory: the canonical architecture map and the
complete document inventory live in `docs/architecture/README.md`. Read the
listed documents in order; consult the conditional ones only if the condition
applies. Retrieve this file only when a change matches one of the recipes
below.

**Changing Task spawning**

1. `docs/architecture/tasks/task-spawn-and-dynamic-graphs.md`
2. `docs/architecture/tasks/blocking-spawn-and-run-yield.md`
3. `docs/architecture/tasks/task-lifecycle.md`

Consult `docs/architecture/execution/agent-control-channel.md` if the worker's
request surface changes, and
`docs/architecture/tasks/taskgraph-dependencies.md` if join semantics change.

**Changing scheduler resource admission**

1. `docs/architecture/scheduling/scheduler-resource-ledger-and-admission.md`
2. `docs/architecture/scheduling/scheduler-reservations-ownership-and-leases.md`

Consult
`docs/architecture/execution/execution-offer-routing-and-admission-handshake.md`
only if the feasibility handshake itself changes,
`docs/architecture/operations/budget-usage-and-rate-limits.md` only if budget
or rate limits are involved, and
`docs/architecture/scheduling/scheduler-dispatch-and-run-intent-reconciliation.md`
only if dispatch commit conditions change.

**Changing persistence or recovery behaviour**

1. `docs/architecture/persistence-and-recovery/sqlite-persistence-and-transactions.md`
2. `docs/architecture/persistence-and-recovery/global-recovery-and-crash-reconciliation.md`
3. `docs/architecture/persistence-and-recovery/recovery-policy.md`

Consult `docs/architecture/persistence-and-recovery/backup-and-restore.md` if
backup completeness, CAS capture, backup verification, or restore-input scope is
involved; `docs/architecture/operations/event-and-observability-model.md` if the
Event Journal or transactional outbox is involved;
`docs/architecture/execution/run-and-attempt.md` if Attempt contact state or
fencing is involved; and
`docs/architecture/security/secret-store-and-credential-brokering.md` if a
SecretProvider, `SecretMutationIntent`, `SecretDescriptor`, credential use, or
`CredentialLease` is involved.

**Changing backup or restore guarantees**

1. `docs/architecture/persistence-and-recovery/backup-and-restore.md`
2. `docs/architecture/persistence-and-recovery/global-recovery-and-crash-reconciliation.md`
3. `docs/architecture/persistence-and-recovery/sqlite-persistence-and-transactions.md`

Consult `docs/architecture/artifacts-and-workspaces/artifact-model.md` if
retention roots, CAS completeness or GC semantics change, and
`docs/architecture/security/secret-store-and-credential-brokering.md` if secret
metadata/provider reconciliation scope changes. Backup payload completeness
never replaces Global Recovery's external-domain fencing.

**Changing execution routing or backend integration**

1. `docs/architecture/execution/execution-fabric.md`
2. `docs/architecture/execution/execution-offer-routing-and-admission-handshake.md`
3. `docs/architecture/execution/run-and-attempt.md`

Consult `docs/architecture/agents-and-context/logical-agent-resolution.md` if
Agent eligibility changes.

**Changing authorization or isolation**

1. `docs/architecture/security/permissions-and-capabilities.md`
2. `docs/architecture/security/sandbox-broker-and-isolation.md`

Consult `docs/architecture/security/secret-store-and-credential-brokering.md`
for credential paths and
`docs/architecture/execution/agent-control-channel.md` for the worker-facing
boundary.

**Changing Goal semantics or completion**

1. `docs/architecture/goals-and-planning/goal-resource.md`
2. `docs/architecture/goals-and-planning/goal-lifecycle-and-completion-controller.md`

Also read
`docs/architecture/evaluation-and-acceptance/evaluation-and-evaluator-registry.md`
whenever Goal acceptance criteria, EvaluationRound/Evidence or evaluator
execution are involved: Goal acceptance runs on the shared evaluation machinery
under the `GOAL_COMPLETION_CANDIDATE` subject type, not on a Goal-private path.

Consult `docs/architecture/goals-and-planning/goal-revision-reconciliation.md`
if revisions are involved and
`docs/architecture/goals-and-planning/planner-and-task-decomposition.md` if
decomposition is involved.

**Changing how results are judged**

1. `docs/architecture/evaluation-and-acceptance/evaluation-and-evaluator-registry.md`
2. `docs/architecture/evaluation-and-acceptance/task-acceptance-and-completion.md`
3. `docs/architecture/artifacts-and-workspaces/artifact-model.md`