//! Architecture-level evidence for Issue #29's pure scheduling vocabulary.

use pantheon_core::config::ComponentDigests;
use pantheon_core::config::Digest;
use pantheon_core::execution::LogicalAgentVersion;
use pantheon_core::scheduling::{
    ContextSourceSnapshot, DispatchMode, ExecutionBinding, GoalFairness, SchedulableTask,
    select_service, service_order,
};

fn goal(id: &str, last_served: Option<i64>) -> GoalFairness {
    GoalFairness {
        goal_id: id.to_string(),
        last_served_sequence: last_served,
    }
}

fn task(goal_id: &str, task_id: &str, eligible_since: i64) -> SchedulableTask {
    SchedulableTask {
        task_id: task_id.to_string(),
        goal_id: goal_id.to_string(),
        eligible_since,
    }
}

#[test]
fn the_least_recently_served_goal_wins_and_a_never_served_goal_sorts_first() {
    // The contract's own example: A=104, C=101, B=97 -> next is B.
    let goals = vec![
        goal("A", Some(104)),
        goal("C", Some(101)),
        goal("B", Some(97)),
    ];
    let tasks = vec![task("A", "a1", 10), task("B", "b1", 5), task("C", "c1", 1)];
    assert_eq!(
        select_service(&goals, &tasks),
        Some(("B".to_string(), "b1".to_string()))
    );
}

#[test]
fn a_never_served_goal_is_least_recently_served_of_all() {
    let goals = vec![goal("served", Some(4)), goal("fresh", None)];
    let tasks = vec![task("served", "s1", 100), task("fresh", "f1", 1)];
    assert_eq!(
        select_service(&goals, &tasks),
        Some(("fresh".to_string(), "f1".to_string()))
    );
}

#[test]
fn equally_unserved_goals_tie_break_on_the_stable_goal_id() {
    let goals = vec![goal("zzz", None), goal("aaa", None)];
    let tasks = vec![task("zzz", "z1", 1), task("aaa", "a1", 9)];
    assert_eq!(
        select_service(&goals, &tasks),
        Some(("aaa".to_string(), "a1".to_string()))
    );
}

#[test]
fn inside_a_goal_the_oldest_eligibility_interval_wins_with_stable_task_tie_break() {
    let goals = vec![goal("g", None)];
    let tasks = vec![
        task("g", "t2", 50),
        task("g", "t1", 40),
        task("g", "t0", 40),
    ];
    assert_eq!(
        select_service(&goals, &tasks),
        Some(("g".to_string(), "t0".to_string())),
        "equal eligible_since falls back to the stable Task id"
    );
}

#[test]
fn selection_does_not_depend_on_input_order() {
    let goals = vec![
        goal("B", Some(97)),
        goal("A", Some(104)),
        goal("C", Some(101)),
    ];
    let tasks = vec![task("C", "c1", 1), task("B", "b1", 5), task("A", "a1", 10)];
    let selected = select_service(&goals, &tasks);
    let mut shuffled_goals = goals.clone();
    shuffled_goals.reverse();
    let mut shuffled_tasks = tasks.clone();
    shuffled_tasks.reverse();
    assert_eq!(
        selected,
        select_service(&shuffled_goals, &shuffled_tasks),
        "the same durable state must select the same pair in any input order"
    );
}

#[test]
fn the_full_order_skips_nothing_and_hides_no_head_of_line() {
    let goals = vec![goal("A", Some(9)), goal("B", None)];
    let tasks = vec![task("A", "a1", 1), task("B", "b1", 2), task("B", "b2", 1)];
    assert_eq!(
        service_order(&goals, &tasks),
        vec![
            ("B".to_string(), "b2".to_string()),
            ("B".to_string(), "b1".to_string()),
            ("A".to_string(), "a1".to_string()),
        ],
        "never-served B precedes served A; inside B, the older interval b2 leads"
    );
}

#[test]
fn no_eligible_tasks_selects_nothing() {
    assert_eq!(select_service(&[goal("A", None)], &[]), None);
    assert_eq!(select_service(&[], &[]), None);
}

#[test]
fn a_goal_without_a_fairness_row_counts_as_never_served() {
    // Fairness rows are created by the first successful service charge, so
    // every fresh Goal's Tasks start with no row at all. An absent row means
    // "never served", which is the most-served-deserving position.
    assert_eq!(
        select_service(&[], &[task("A", "a1", 1)]),
        Some(("A".to_string(), "a1".to_string()))
    );
}

#[test]
fn dispatch_mode_wire_spelling_round_trips_exactly() {
    for mode in [DispatchMode::Running, DispatchMode::Paused] {
        assert_eq!(DispatchMode::parse(mode.as_str()), Some(mode));
    }
    assert_eq!(DispatchMode::parse("running"), None);
    assert_eq!(DispatchMode::parse(""), None);
}

fn binding() -> ExecutionBinding {
    ExecutionBinding {
        task_id: "task-1".to_string(),
        agent: LogicalAgentVersion {
            name: "builder".to_string(),
            version: 3,
        },
        request_digest: Digest::of(b"request"),
        offer_digest: Digest::of(b"offer"),
        backend_id: "fake-local".to_string(),
        descriptor_revision: 7,
        descriptor_digest: Digest::of(b"descriptor"),
        execution_profile_digest: Digest::of(b"profile"),
        sandbox_profile_digest: Digest::of(b"sandbox"),
        route_policy_digest: Digest::of(b"policy"),
        configuration_activation_sequence: 11,
        configuration_content_digest: Digest::of(b"revision"),
        component_digests: ComponentDigests {
            agents: Digest::of(b"agents"),
            routing: Digest::of(b"routing"),
            execution_profile: Digest::of(b"exec"),
            evaluator_registry: Digest::of(b"eval"),
            context_policy: Digest::of(b"context"),
            authorization: Digest::of(b"authz"),
        },
    }
}

#[test]
fn a_binding_digest_covers_every_frozen_field_not_a_chosen_few() {
    let frozen = binding();

    // Each field below carries strategy authority; changing any one must
    // change the Binding identity. This is the same shape as #28's
    // ConfigurationBinding mutant, applied to the whole freeze surface.
    let mut variants = Vec::new();
    for mutate in [
        |b: &mut ExecutionBinding| b.task_id = "task-2".to_string(),
        |b: &mut ExecutionBinding| b.agent.version = 4,
        |b: &mut ExecutionBinding| b.request_digest = Digest::of(b"other"),
        |b: &mut ExecutionBinding| b.offer_digest = Digest::of(b"other"),
        |b: &mut ExecutionBinding| b.backend_id = "other".to_string(),
        |b: &mut ExecutionBinding| b.descriptor_revision = 8,
        |b: &mut ExecutionBinding| b.descriptor_digest = Digest::of(b"other"),
        |b: &mut ExecutionBinding| b.execution_profile_digest = Digest::of(b"other"),
        |b: &mut ExecutionBinding| b.sandbox_profile_digest = Digest::of(b"other"),
        |b: &mut ExecutionBinding| b.route_policy_digest = Digest::of(b"other"),
        |b: &mut ExecutionBinding| b.configuration_activation_sequence = 12,
        |b: &mut ExecutionBinding| b.configuration_content_digest = Digest::of(b"other"),
        |b: &mut ExecutionBinding| b.component_digests.authorization = Digest::of(b"other"),
        |b: &mut ExecutionBinding| b.component_digests.context_policy = Digest::of(b"other"),
    ] {
        let mut variant = frozen.clone();
        mutate(&mut variant);
        variants.push(variant);
    }

    for variant in variants {
        assert_ne!(
            variant.digest(),
            frozen.digest(),
            "a mutated {variant:?} must not keep the frozen digest"
        );
    }
}

#[test]
fn the_binding_canonical_form_reproduces_its_own_digest() {
    let frozen = binding();
    let canonical = String::from_utf8(frozen.to_value().to_canonical_bytes()).expect("utf-8");
    assert_eq!(Digest::of(canonical.as_bytes()), frozen.digest());
}

#[test]
fn a_source_snapshot_digest_covers_every_named_source_generation() {
    let snapshot = ContextSourceSnapshot {
        task_spec_digest: Digest::of(b"spec"),
        goal_id: "goal-1".to_string(),
        goal_revision: 8,
        graph_revision: 47,
        agent: LogicalAgentVersion {
            name: "builder".to_string(),
            version: 1,
        },
        configuration_activation_sequence: 43,
        context_policy_digest: Digest::of(b"context-policy"),
        workspace_id: "ws-1".to_string(),
        workspace_resolved_base: "a".repeat(40),
    };
    let canonical = String::from_utf8(snapshot.to_value().to_canonical_bytes()).expect("utf-8");
    assert_eq!(Digest::of(canonical.as_bytes()), snapshot.digest());

    let mut moved_goal = snapshot.clone();
    moved_goal.goal_revision = 9;
    assert_ne!(moved_goal.digest(), snapshot.digest());

    let mut moved_base = snapshot.clone();
    moved_base.workspace_resolved_base = "b".repeat(40);
    assert_ne!(moved_base.digest(), snapshot.digest());
}
