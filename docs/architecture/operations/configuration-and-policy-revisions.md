# Configuration and Policy Revisions

## Status

Canonical Pantheon configuration publication and policy revision specification.

## Purpose

Pantheon requires one coherent, immutable, auditable configuration snapshot for authorization, routing, evaluator publication, execution profiles, recovery policy, context construction, credential binding resolution, and other operator-controlled registries. Controllers must never observe partially applied configuration or infer security authority directly from mutable source files.

The central rule is:

> **Source files are desired configuration inputs. Runtime authority is an immutable, validated ConfigurationRevision stored in Pantheon state and activated atomically.**

## 1. ConfigurationRevision

A ConfigurationRevision is immutable and contains a manifest over independently immutable domain components.

Conceptually:

```yaml
configurationRevision:
  id: cfgrev_01K...
  activationSequence: 42
  digest: sha256:CONFIG

  components:
    authorization: sha256:AUTH
    routing: sha256:ROUTE
    evaluators: sha256:EVAL
    executionProfiles: sha256:EXEC
    recoveryPolicies: sha256:RECOVERY
    agents: sha256:AGENTS
    context: sha256:CONTEXT
    credentialBindings: sha256:CREDENTIAL_BINDINGS

  sourceSetDigest: sha256:SOURCES
  compiler:
    version: pantheon-config-v1
```

Revision identity is historical activation identity. Content digest is semantic content identity. Rolling back to identical content creates a new ConfigurationRevision with a later activation sequence.

## 2. Domain-specific component digests

Pantheon must not use one ambiguous `policyHash` for unrelated domains.

Use explicit bindings such as:

```text
configRevision
authzPolicyDigest
routePolicyDigest
recoveryPolicyDigest
admissionPolicyDigest
integrationPolicyDigest
evaluatorRegistryDigest
executionProfileDigest
contextPolicyDigest
credentialBindingRegistryDigest
```

`contextPolicyDigest` is the digest of the immutable `context` ConfigurationRevision component used by Context Builder decisions. It is not an authorization digest.

`credentialBindingRegistryDigest` is the digest of the immutable `credentialBindings` ConfigurationRevision component. It identifies compiled logical credential-mapping authority only; it never hashes or embeds SecretVersionId values or secret material.

Decision records bind the exact configuration component that affected them.

## 3. Configuration composition is semantic

Configuration scopes may include built-in/system, installation/user, project, and Agent inputs, but composition is domain-specific.

Do not use generic deep YAML merging to decide authority.

Examples:

- authorization uses hard-policy/default-deny/forbid and scoped-policy semantics;
- routing resolves named route policies deterministically;
- evaluator registry resolves logical refs to immutable versions;
- execution profiles use explicitly defined inheritance/composition;
- Agent configuration follows the Agent Manifest layering contract;
- context configuration compiles deterministic mandatory/preload/drop/retrieval limits into one immutable ContextPolicy component;
- credential-binding configuration compiles canonical action/resource scope plus logical SecretRef, broker mechanism class and credential-use constraints into one immutable CredentialBindingRegistry component.

Goal/Task restrictions and temporary Grants are runtime state, not ConfigurationRevision source configuration.

## 4. Built-in hard policy

Pantheon has a compiled-in hard-security base that operator/project configuration cannot weaken.

Examples include:

- workers cannot access Pantheon database/state directly;
- workers cannot access policy storage or operator control sockets;
- workers cannot obtain privileged host runtime sockets by default;
- agents cannot self-approve or self-escalate authority.

The built-in hard-policy version participates in the AuthorizationComponent digest.

## 5. AuthorizationComponent

Authorization configuration compiles into a validated immutable component containing the effective Cedar schema/policy set and Pantheon hard policy identity.

Conceptually:

```yaml
authorizationComponent:
  schemaVersion: 1
  cedar:
    schemaDigest: sha256:...
    policySetDigest: sha256:...
  hardPolicyDigest: sha256:...
  compilerVersion: ...
  digest: sha256:AUTH
```

All Cedar policy/schema combinations must parse and validate before activation. Invalid authorization configuration never becomes active.

## 6. ContextComponent

Context construction policy is an independently immutable ConfigurationRevision component.

Conceptually:

```yaml
contextComponent:
  schemaVersion: 1
  mandatorySections: ...
  preloadPriority: ...
  memoryLimits: ...
  workspaceOrientationLimits: ...
  safetyMargin: ...
  optionalDropOrder: ...
  compilerVersion: ...
  digest: sha256:CONTEXT
```

Its canonical decision digest is named `contextPolicyDigest`.

The ContextComponent controls deterministic semantic selection/trimming only. It is not a frozen security-authority grant: current hard/current authorization policy remains independently enforceable while a Run is preparing/executing.

CredentialBindingRegistry is likewise independently immutable ConfigurationRevision state. Each compiled binding has a canonical `credentialBindingAuthorityDigest` over the semantic action/resource scope, logical SecretRef, broker mechanism class and credential-use constraints. The digest deliberately excludes SecretVersionId and secret bytes so material rotation behind the same logical SecretRef does not change the binding authority identity.

## 7. Canonical hashing

Pantheon hashes canonical compiled semantics, not only source bytes.

Maintain both:

```text
sourceSetDigest
compiled component/configuration digest
```

The source digest records what the operator supplied. The compiled digest records what Pantheon actually uses.

Hash-bearing canonical historical documents are versioned and never rewritten in place by migrations.

## 8. Compile-before-activate

A candidate configuration is fully prepared before active state changes.

Pipeline:

```text
read source bytes
→ parse
→ schema validate
→ domain-specific composition
→ compile Cedar/policies/context policy/credential bindings
→ validate all cross-references
→ resolve evaluator/route/profile/Agent/credential-binding refs
→ canonicalize
→ hash
→ candidate ConfigurationRevision
```

Only a fully valid candidate may activate.

## 9. Atomic activation

Active state contains one small pointer to the current immutable ConfigurationRevision.

Conceptually:

```text
active_configuration
  singleton
  config_revision_id
  status_revision
```

Activation performs one SQLite transaction:

```text
insert immutable revision/components if needed
update active_configuration pointer
append ConfigurationActivated event
```

No component activates independently.

## 10. Configuration publication barrier

Pantheon must prevent requests from observing a database-active new revision while process-local compiled configuration still points at the old revision.

Activation therefore uses a short publication barrier:

```text
compile immutable cfgrev N
→ acquire publication barrier
→ commit active pointer = N
→ atomically publish the same in-memory snapshot
→ release barrier
```

During the barrier, no new authorization decision, Scheduler Run commitment, or Agent Control request admission may begin.

If Pantheon crashes after DB commit but before process publication, restart loads the DB-authoritative active revision before serving work.

## 11. Controller snapshot rule

Every consequential controller operation captures one ConfigurationRevision and uses it consistently throughout the operation.

Example Scheduler cycle:

```text
capture cfgrev 43
→ Ready validation
→ Agent resolution
→ ExecutionRequest
→ offer validation
→ route policy
→ admission
→ construct/canonicalize ContextSourceSnapshot inputs under cfgrev 43
→ immediately before T3 commit verify cfgrev 43 is still active
```

If active configuration changed, the operation aborts and is recomputed under the new snapshot.

T3 binds the exact `ConfigurationRevision + contextPolicyDigest` into the Run's immutable ContextSourceSnapshot. Context Builder later uses that frozen source snapshot even if another ConfigurationRevision activates while preparation is in progress.

T3 also freezes the exact `credentialBindingRegistryDigest` into the immutable ExecutionBinding. This freezes which logical credential-mapping authority may satisfy later brokered operations for that Run without freezing the SecretVersion/material that may be current when the operation actually executes.

## 12. Immutable decision binding

ExecutionBindings and other immutable decisions record the exact configuration revision/components used.

Examples:

```yaml
ExecutionBinding:
  configRevision: cfgrev_43
  routePolicyDigest: sha256:R
  executionProfileDigest: sha256:E
  authzCeilingDigest: sha256:A
  credentialBindingRegistryDigest: sha256:CBR
```

```yaml
ContextSourceSnapshot:
  configRevision: cfgrev_43
  contextPolicyDigest: sha256:CONTEXT
```

```yaml
RecoveryDecision:
  configRevision: cfgrev_43
  recoveryPolicyDigest: sha256:RP
```

```yaml
EvaluationRound:
  configRevision: cfgrev_43
  evaluatorRegistryDigest: sha256:ER
```

For credential-bearing actions, `credentialBindingRegistryDigest` is immutable decision provenance. At use time Pantheon resolves the exact action/resource binding from that frozen registry and separately from the current active registry. The exact resolved `credentialBindingAuthorityDigest` must still match before secret retrieval; an unrelated registry change does not invalidate the Run.

## 13. Existing Runs and authorization changes

A Run keeps its frozen authorization ceiling, while current active authorization policy can further restrict it.

For a live Run:

```text
effective current authority
=
frozen Run authorization ceiling
∩ current active authorization policy
∩ current Goal/Task restrictions
+ valid scoped Grants within those ceilings
```

Consequences:

- tightening current policy takes effect for future actions;
- relaxing policy does not silently broaden an already-running Run;
- temporary Grants remain constrained by hard policy and the frozen Run ceiling.

Pantheon does not need to prove whether an arbitrary policy change is semantically a pure tightening or relaxation.

Credential mapping is an additional independent gate for credential-bearing operations. Current configuration may remove or change a binding and thereby deny an existing Run, but it may not remap that Run onto a different credential authority. Existing Runs compare the exact frozen/current resolved binding authority, not whole-registry equality.

## 14. Physical enforcement on policy tightening

A policy update can only be considered enforced if the execution environment can physically enforce the new restriction.

If a sandbox can dynamically tighten the required restriction, update it and continue.

If not, Pantheon must stop/finalize the affected Run and require future work to execute under a compatible environment.

Pantheon must never claim a security policy is active for an execution whose physical sandbox cannot enforce it.

## 15. Routing, evaluator, execution-profile, Agent, context, and credential-binding changes

Normal preference/configuration changes affect future work only unless current security/credential use rules deny an existing action.

- route policy change: existing Binding unchanged; future Runs use new route policy;
- evaluator registry logical ref change: existing Tasks retain their pinned EvaluatorVersion; future Tasks resolve the new version;
- execution profile change: existing Run keeps frozen profile unless current hard security invalidates it;
- Agent definition change: existing Run keeps frozen Agent snapshot; future Agent Resolution uses the active version;
- context policy change: existing Run keeps the `contextPolicyDigest` frozen in its ContextSourceSnapshot; future Runs freeze the newly active ContextComponent;
- credential binding change: existing Run keeps its frozen `credentialBindingRegistryDigest`; a credential-bearing action is permitted only if the exact current binding still resolves to the same `credentialBindingAuthorityDigest`; unrelated binding changes do not invalidate the Run, while a changed/removed exact binding denies the action.

A context policy update never causes Context Builder to rebuild/replace the ContextPlan of an already committed Run. Material semantic context changes require a new Run.

Credential material rotation behind the same logical SecretRef is not a CredentialBinding change and does not require a new Run solely because `SecretVersionId` changed.

## 16. Backend registration and draining

Removing or disabling a backend from configuration means no new work may route to it. It does not immediately destroy the adapter/recovery capability required by existing Attempts.

Lifecycle concept:

```text
active for offers
→ draining / unavailable for new work
→ existing obligations reconciled
→ safe unload/removal
```

Backend health remains runtime operational state, not ConfigurationRevision content.

## 17. Invalid candidates and last-known-good configuration

Invalid configuration candidates are rejected atomically and never partially apply.

The previous valid ConfigurationRevision remains active.

Pantheon records rejected load attempts and diagnostics for operator inspection.

## 18. Explicit apply in v1

Authority-sensitive configuration activation is explicit in v1.

Recommended operator flow:

```text
pantheon config validate
pantheon config diff
pantheon config apply
```

Filesystem watching may detect source drift, but must not automatically activate arbitrary editor writes.

This avoids half-written files becoming temporary authority.

## 19. Startup and source drift

On daemon startup Pantheon loads the durable active ConfigurationRevision from SQLite, verifies the component hashes, and reconciles source drift separately.

If source files differ from the active source set, report drift; do not silently activate it.

Fresh installation is the exception: before Pantheon can become ready, one valid initial ConfigurationRevision must be compiled and activated.

## 20. Configuration diagnostics

Useful operational conditions include:

```text
ACTIVE
DRIFTED
REJECTED_CANDIDATE
RECONCILING_SECURITY
```

These are conditions/diagnostics, not lifecycle phases of immutable ConfigurationRevision objects.

## 21. Source provenance

Every ConfigurationRevision records the logical source inputs and digests that produced it.

Filesystem path/location is provenance only, not semantic identity.

Configuration may contain SecretRefs but never raw secret values or SecretVersion material identity as part of CredentialBinding authority.

## 22. Rollback

Rollback never moves activation history backward.

`pantheon config rollback cfgrev_40` means:

```text
revalidate compatibility/current hard policy
→ create a new later ConfigurationRevision activation using equivalent compiled content
→ activate it atomically
```

Historical revision rows remain immutable.

## 23. Configuration reconciliation

After activation, controllers are awakened but Events are only hints. Each controller re-reads the current active ConfigurationRevision and reconciles its subjects.

A controller may record which ConfigRevision it last evaluated for a subject. Stale reconciliation work is discarded if a newer revision is already active.

The Scheduler may not commit a new Run until it has observed the active ConfigRevision.

An existing Run whose T3 already committed does not switch ContextPolicy during preparation; Context Builder resolves policy through the Run's ContextSourceSnapshot rather than the current active pointer.

A credential-bearing broker operation does not switch the Run's frozen credential authority either. It compares the exact binding resolved from the Run's frozen CredentialBindingRegistry with the exact binding under the current active registry before material retrieval.

## 24. Persistence model

Likely persistence families:

```text
configuration_components
configuration_revisions
active_configuration
configuration_sources
config_load_attempts
config_reconciliations
```

`configuration_components` includes the immutable ContextComponent addressed by `contextPolicyDigest` and the immutable CredentialBindingRegistry addressed by `credentialBindingRegistryDigest`; neither component contains secret material. ContextSourceSnapshot/ContextPlan persistence belongs to the Run/context architecture rather than configuration publication state.

Exact DDL is implementation work.

## 25. Operator API/CLI

The operator control surface should support operations equivalent to:

```text
pantheon config status
pantheon config validate
pantheon config diff
pantheon config apply
pantheon config history
pantheon config show <revision>
pantheon config rollback <revision>
```

Agent Control has no configuration-management operations.

## 26. Excluded operational state

ConfigurationRevision is not a snapshot of the whole database. It excludes runtime state such as:

- Goals/Tasks/Runs/Attempts;
- ContextSourceSnapshots/ContextPlans attached to Runs;
- SecretVersionId/current secret material state;
- ResourceReservations/BudgetHolds/Usage;
- backend health/rate limits;
- temporary Grants/Approvals;
- RecoveryFindings;
- workspaces;
- Event/export cursors.

## Core invariants

1. Source files are inputs; active immutable ConfigurationRevision is runtime configuration authority.
2. Configuration activates as one coherent snapshot, never component-by-component.
3. Configuration composition is domain-specific and security policy is never decided by generic deep merge.
4. Invalid configuration never partially applies; last-known-good remains authoritative.
5. Built-in hard policy cannot be weakened by lower configuration scopes.
6. Controllers bind one ConfigRevision per consequential operation and revalidate staleness before commit.
7. Immutable decisions bind exact component digests rather than one ambiguous `policyHash`.
8. Context construction has an explicit immutable `context` component named by `contextPolicyDigest`; it is never hidden behind a generic policy field.
9. Credential mapping has an explicit immutable `credentialBindings` component named by `credentialBindingRegistryDigest`; compiled binding authority excludes SecretVersionId/secret material.
10. T3 freezes `configRevision + contextPolicyDigest` into the Run's ContextSourceSnapshot and freezes `credentialBindingRegistryDigest` into ExecutionBinding.
11. Existing Runs cannot gain authority from a later policy relaxation or credential-binding remap; exact credential use requires frozen/current resolved binding-authority equality.
12. Current policy tightening applies to future actions and may force Run termination if physical enforcement cannot be tightened safely.
13. Credential material may rotate behind the same logical SecretRef without broadening the Run or requiring a new Run solely for the new SecretVersionId.
14. Configuration history and hash-bearing historical documents are immutable.
15. Explicit configuration activation is operator authority and unavailable to Agent Control.
16. A single-daemon/single-database Pantheon uses atomic coherent configuration snapshots rather than eventually consistent component rollout.
