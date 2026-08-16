# Scheduler Resource Ledger and Admission

## Status

Canonical Pantheon reservable-resource admission specification.

## Purpose

The Resource Ledger answers whether a candidate workload can reserve a whole compatible set of finite capacity **now** without embedding provider/model semantics.

> **Resource capacity is represented by generic namespaced resource keys and integer quantities. Reservations are authority to consume capacity; observed utilization is not admission authority.**

## Separate ledgers

```text
RESOURCE LEDGER    reversible capacity/concurrency
BUDGET LEDGER      cumulative allowance + Holds
USAGE LEDGER       factual consumption/Charges
RATE-LIMIT STATE   temporary replenishing upstream availability
```

They may jointly affect scheduling but are never one generic quota object.

## Resource model

A resource descriptor contains at least:

```text
ResourceKey
allocationMode = DIVISIBLE | DISCRETE
unit
capacity
allocatable
health
revision
observedAt
```

Core treats ResourceKey as namespaced/opaque. Examples may include host CPU/memory, Workspace disk/slot, Sandbox slot, backend concurrency and synthetic global/Goal Run concurrency.

No scheduler branch is keyed to a concrete provider/model/harness name.

## Effective desired claim set

For one Agent+ExecutionOffer candidate Pantheon computes the desired effective claims from all layers:

```text
Task/Agent requirements
+ ExecutionOffer factual resource needs
+ Workspace requirements
+ SandboxPlan requirements
+ scheduler/policy synthetic limits
```

This is the **desired ownership state**, not automatically the set of new Reservations to create.

## Incremental claim set

Before admission, Pantheon subtracts compatible capacity already durably owned by the same legitimate holder scope.

For a Task with an existing Task-scoped Workspace reservation:

```text
incremental claims
  = desired effective claims
    - compatible Task-scoped reservations already held by this Task
```

If desired quantity increases, request only the positive delta or perform an explicit resize. If identity/semantics are incompatible, Admission cannot silently treat old capacity as satisfying new requirements.

This prevents requeue/new Runs from reserving another Workspace slot each time.

Run-scoped claims are normally fresh for the new Run because backend/Sandbox/concurrency strategy may differ.

## Holder scopes

ResourceReservations may be owned by:

```text
Task
Run
control-operation
```

Task scope is for durable capacity deliberately surviving Runs. Run scope is execution-strategy capacity. Evaluation and similar bounded control work use control-operation scope.

## Whole-set admission

Admission is all-or-nothing for the incremental claim set. Pantheon never commits a partially reserved Run intent that assumes missing capacity will appear later.

Pure assessment result:

```text
ADMITTABLE
TEMP_UNAVAILABLE
UNSATISFIABLE
```

### ADMITTABLE

Current descriptor revisions/allocatable capacity can satisfy all incremental claims.

### TEMP_UNAVAILABLE

Claims are semantically satisfiable but currently occupied/unhealthy/freshness-fenced. Task stays Ready; scheduler waits/retries when state changes.

### UNSATISFIABLE

No allowed current configuration/resource shape can satisfy the hard claim; this feeds structured scheduling/recovery rather than busy retry.

## Capacity arithmetic

For each resource key, admission considers non-released Reservations, not utilization estimates:

```text
reserved + incremental_requested <= allocatable
```

subject to allocation mode/discrete identity rules.

V1 has no overcommit. If later overcommit exists it must be an explicit separate policy, not accidental arithmetic.

## Capacity publishers

Resource facts may be published by controller-owned observers such as:

```text
host
Workspace Controller
Sandbox Controller
ExecutionBackend registry/adapter
scheduler synthetic policy
```

Publishers report factual capacity/health/revision. A backend cannot grant itself authorization or a favorable routing score by publishing "quality" as a resource.

## Capacity shrink

If allocatable shrinks below existing Reservations, current Reservations remain charged. New admission is blocked; Pantheon does not revoke live capacity merely to restore arithmetic.

Recovery/operator action may later drain/stop work explicitly.

## Reservation transaction boundary

Assessment itself is side-effect-free. T3 Scheduler commit re-reads current resource revisions and existing Task reservations, then atomically creates/activates only the required incremental Reservations together with Binding/Run/Holds/Task Active.

Race between two candidate admissions is therefore resolved by serialized authoritative write/revalidation, not optimistic in-memory accounting.

## Evaluation/control operations

EvaluationOperation uses the same Ledger rather than creating unaccounted verification work. Its controller requests control-operation claims and commits Reservations before external verification Sandbox/process provisioning.

It does not use Agent Resolution/Task fairness merely because it uses the same capacity ledger.

## Sandbox resources

SandboxPlan contributes factual resource claims such as:

```text
sandbox.container.slot
sandbox.vm.slot
sandbox.disk.bytes
memory.bytes
cpu.*
```

The Sandbox cannot be provisioned outside accounted capacity simply because it is "preparation" rather than the executor itself.

## UNKNOWN

Reservations protecting UNKNOWN external obligations remain consumed/UNCERTAIN. Lease expiry, timeout or daemon restart does not make capacity free.

Exceptional operator force-resolution may explicitly tombstone/fence the old lineage and settle/release capacity with a high-severity Audit record; this is not automatic admission behavior.

## Persistence invariants

Where a resource family defines one logical Task reservation per key, persistence enforces at most one non-released Task reservation for:

```text
(task_id, resource_key)
```

Controller logic additionally validates quantity and resource revision/identity compatibility.

Run live-concurrency/resources are separately keyed to Run ownership.

## Core invariants

1. Resource Ledger contains generic resources, not provider-specific business logic.
2. Desired effective claims and incremental claims are distinct.
3. Existing compatible Task-scoped Reservations satisfy the corresponding desired claim and are not recreated on every Run.
4. Whole incremental claim set must fit before authoritative Run admission.
5. Reservation authority, utilization, Budget, Usage and Rate Limits are different concepts.
6. Capacity shrink keeps existing reservations charged and blocks incompatible new admission.
7. Evaluation/Sandbox capacity is accounted through the same Ledger.
8. UNKNOWN capacity remains reserved until reconciled or explicitly force-resolved.
