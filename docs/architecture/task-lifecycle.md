# Task Lifecycle and State Machine

## Status

Canonical Pantheon Task lifecycle specification.

## Purpose

`TaskSpec` is immutable semantic intent. `TaskStatus` is controller-owned mutable execution state. The Task lifecycle must remain correct across Run/Attempt failure, blocking child work, Acceptance, cancellation, Goal revision, recovery and daemon restart.

See also:

- `run-and-attempt.md`
- `blocking-spawn-and-run-yield.md`
- `task-acceptance-and-completion.md`
- `recovery-policy.md`
- `goal-revision-reconciliation.md`

## Foundational invariants

1. **Only Pantheon transitions Task status.** Workers submit semantic requests; they never write lifecycle state.
2. **Attempt failure is not Run failure; Run failure is not Task failure; Task failure is not Goal failure.**
3. **A Task has at most one nonterminal responsible Run.**
4. **`Task Active` means exactly one nonterminal Run owns execution responsibility.**
5. **`Task Ready` and `Task Waiting` mean zero nonterminal Runs.**
6. **Waiting is durable and consumes no executor Run slot.**
7. **Success requires immutable Candidate acceptance and finalization.**
8. **Cancellation and supersession never terminalize a Task while a responsible Run is still nonterminal.**
9. **`Task Finalizing` always carries a durable `terminalTarget`; restart never reconstructs terminal intent from Events or surrounding state.**
10. **Terminal Tasks never reopen.** Further semantic work requires another Task.
11. **All lifecycle writes use revision/CAS checks and append a durable Event in the same authoritative transaction.**

## v1 phases

Nonterminal:

```text
Pending
Ready
Active
Waiting
Evaluating
Finalizing
```

Terminal:

```text
Succeeded
Failed
Cancelled
Superseded
```

## Pending

The Task exists but logical prerequisites are not yet satisfied, for example dependencies or graph activation gates.

Resource/backend scarcity does not normally make a logically runnable Task Pending; such a Task is Ready and waits for scheduling/admission.

## Ready

The Task is logically eligible for scheduling and owns no nonterminal Run.

The Scheduler is the only component that may create a scheduled Run for a Ready Task. Resource/backend/budget feasibility is decided after logical readiness.

## Active

Exactly one nonterminal Run is responsible for progressing the Task.

`Active` does not assert that an OS process is currently running; preparation, Attempt reconciliation, UNKNOWN external state and sequential execution retries remain Run/Attempt concerns.

## Waiting

The Task remains live but must not hold executor Run capacity.

V1 authoritative Waiting reason is primarily `ChildJoin`; future durable external/human wait reasons may reuse the same ownership rule.

For a blocking child:

```text
Task Active
  ↓ blocking spawn accepted
responsible Run Finalizing / terminalTarget=Yielded
  ↓ execution safely stopped and Run-scoped resources settled
Run Yielded
  ↓
Task Waiting / ChildJoin
  ↓ child accepted + join satisfied
Task Ready
  ↓
Scheduler may create a new Run
```

A Waiting Task has **zero** nonterminal Runs. The old Run is never resumed. Continuation is represented durably through the Task Workspace checkpoint, join state and accepted child Artifact bindings; a later Run receives a new frozen ContextPlan.

If termination of the yielding Run is UNKNOWN, the Task remains Active and the Run remains Finalizing. Pantheon does not enter Waiting or create a replacement Run until the old execution obligation is safe.

## Evaluating

A CandidateResult has been durably submitted and frozen. Pantheon evaluates the Task acceptance contract through the Evaluation subsystem.

The producing Run may still be Finalizing while Task acceptance executes. Evaluation completion does not permit another Run until the prior Run is terminal.

A rejected Candidate remains immutable history. Recovery may later return the Task to Ready, but only after the producing Run has become terminal and the recovery/requeue transaction revalidates ownership.

## Finalizing

Pantheon has selected a terminal Task target and is satisfying idempotent finalization obligations before the terminal phase is committed.

Conceptually:

```yaml
status:
  phase: Finalizing
  terminalTarget:
    outcome: Succeeded | Failed | Cancelled | Superseded
    reason: ...
  revision: ...
```

`terminalTarget` answers **what terminal outcome Pantheon has already selected**. Finalization obligations answer **what must become safely true before that outcome may be committed**. The two are not inferred from one another.

Typical obligations include:

- ensure the responsible Run/Attempt is safely terminal or fenced;
- stop further scheduling/spawn authority for this Task;
- settle/release eligible reservations and BudgetHolds;
- freeze/preserve Workspace/Artifact state required by the outcome;
- notify graph joins/dependents;
- establish final retention/finalizer state.

Most obligations are reconstructed after restart from the authoritative domain rows that already own the fact: Run/Attempt status, Sandbox status/tombstone, ResourceReservation, BudgetHold/Usage, WorkspaceRevision, Artifact retention, IntegrationIntent and related controller state. Pantheon does not duplicate those facts into a second generic finalizer ledger merely to mirror them.

If a finalization action has independent retry/uncertainty state that is not durably represented by another owning domain object, Pantheon records an explicit durable finalization obligation for that action. Such an obligation carries its own stable key/state and is reconciled idempotently; it never replaces the owning domain's authoritative state.

A Task may leave `Finalizing` only when its durable `terminalTarget` is present, every required authoritative domain predicate is safe for that target, and every explicit finalization obligation is satisfied or otherwise resolved under Recovery Policy.

Finalization is restart-safe. A crash never requires guessing which terminal state was intended because `terminalTarget` is durable, and cleanup progress is derived only from durable authoritative state or explicit durable obligation records—not from Event narration or process memory.

## Terminal phases

### Succeeded

The current Candidate satisfied the Task acceptance contract and finalization completed.

### Failed

Pantheon determined that no permitted recovery path remains or the Task cannot/should not continue toward its contract.

### Cancelled

A trusted authority intentionally stopped the Task and finalization safely closed its obligations.

### Superseded

The Task is no longer authoritative because Goal/Graph reconciliation replaced it with newer work. Supersession preserves history and is not failure.

Terminal Tasks are immutable and never reopen.

## Candidate submission

`task.submit_result` is an Agent Control semantic request. Submission may commit only if all authoritative preconditions are still true, including:

```text
AgentControlSession ACTIVE
Attempt is the current nonterminal Attempt of the Run
Run.phase == Active
Task.phase == Active
Task.activeRun == this Run
expected Task status revision matches
no cancellation/supersession/finalization fence already committed
Candidate structure/Artifact refs are valid
```

The authoritative submission transaction re-reads these facts, performs revision CAS, records the immutable Candidate, moves `Run Active -> Finalizing`, moves `Task Active -> Evaluating`, and appends Events.

### Cancellation precedence

**Cancellation/supersession wins if its authoritative state transition commits first.** A later `task.submit_result` receives a conflict/stale-authority result and cannot create the current Candidate.

If Candidate submission commits first, that Candidate remains immutable historical truth; a later cancellation may still move the Task toward Cancelled, but it never rewrites or deletes the Candidate.

## Acceptance rejection and requeue

Acceptance rejection itself does not fail or reopen the producing Run.

Normal sequence:

```text
Candidate rejected
  ↓
RecoveryDecision recorded
  ↓
if producing Run still nonterminal:
    Task remains Evaluating (condition: PriorRunFinalizing)
  ↓
producing Run terminal
  ↓
REQUEUE_TASK transaction revalidates current Goal/Task/Graph/policy
  ↓
Task -> Ready
```

`REQUEUE_TASK` **must not** set Ready while the prior Run remains nonterminal. This preserves the one-live-Run invariant and avoids a scheduler deadlock against the unique-live-Run constraint.

The next Scheduler decision creates a new Run/new Binding because Acceptance evidence changes semantic execution context.

## Cancellation

Any nonterminal Task may be driven toward cancellation by setting:

```text
phase = Finalizing
terminalTarget = Cancelled
```

If a responsible Run exists, its desired execution becomes stopped and it must safely terminalize/fence before the Task becomes Cancelled.

A Waiting Task has no Run, so cancellation primarily handles attached child propagation, Workspace/finalizer cleanup and accounting obligations.

## Supersession

Goal reconciliation never changes an Active Task directly to terminal `Superseded`.

Correct path:

```text
Active/Evaluating/Waiting/etc.
  ↓
Finalizing / terminalTarget=Superseded
  ↓
stop/fence responsible execution where applicable
settle finalization obligations
  ↓
Superseded
```

This prevents terminal Tasks from coexisting with nonterminal Runs and avoids manufacturing recovery quarantine findings.

## Conditions

Operational detail remains orthogonal conditions rather than phase proliferation. Useful examples include:

```text
DependenciesSatisfied
ChildJoinSatisfied
AcceptanceSatisfied
PriorRunFinalizing
RunHealthy
PolicySatisfied
Blocked
```

Condition state may be `True | False | Unknown` and records the Task/Goal/Graph revision it observed where relevant.

## Transition authority and idempotency

Workers may request operations such as:

```text
task.submit_result
task.spawn
task.cancel.requested
```

Pantheon validates them through Agent Control and controller state; workers do not set `status.phase`.

Every mutating command/request has stable idempotency identity and authoritative writes use expected revision/CAS. Losing concurrent operations re-read and re-evaluate rather than overwrite the winner.

## Restart reconciliation

Pantheon reconciles every nonterminal Task after restart. Examples:

```text
Active + Run nonterminal
→ reconcile that Run

Waiting / ChildJoin + join satisfied
→ Ready (never resume old Run)

Evaluating + evaluation incomplete
→ reconcile EvaluationRound

Evaluating + rejected + prior Run still finalizing
→ remain Evaluating

Finalizing + terminalTarget present
→ recompute required finalization predicates from durable owning-domain state
→ reconcile any explicit durable finalization obligations
→ continue toward that exact terminalTarget
```

A `Finalizing` Task without a durable `terminalTarget` is an invariant violation and is quarantined; Recovery does not infer the intended outcome from Events, Candidate state or surrounding objects.

Waiting relationships never depend on an in-memory future/model process.

## v1 invariants

1. `Active -> Succeeded` is illegal.
2. Success passes through `Evaluating -> Finalizing -> Succeeded`.
3. `Task Active => exactly one nonterminal Run`.
4. `Task Ready|Waiting => zero nonterminal Runs`.
5. Blocking spawn yields/terminalizes the old Run before Task enters Waiting.
6. UNKNOWN old execution prevents Waiting/requeue/replacement execution.
7. Candidate submission is revision-bound and loses to an already-committed cancellation/supersession fence.
8. Acceptance rejection cannot return the Task to Ready until its producing Run is terminal.
9. Cancellation/supersession use Finalizing + terminalTarget and never leave a terminal Task with a live Run.
10. `Task Finalizing => terminalTarget is durably present`; finalization completion is derived only from durable authoritative domain state plus any explicit durable obligation rows.
11. Terminal Tasks never reopen.