//! `pantheond`: the Pantheon daemon and the workspace's composition root.
//!
//! # Owns
//!
//! Wiring, and only wiring. Process bootstrap, observability initialization,
//! constructing the store and engine, choosing the concrete backends that
//! satisfy the engine's abstract ports, starting the operator server,
//! supervising controllers, and orderly shutdown.
//!
//! Being the single place that names concrete implementations is what lets
//! every other crate stay independent of them.
//!
//! # Must not own
//!
//! Domain rules, persistence details, orchestration logic or request handling.
//! Anything worth testing without starting a process belongs in the crate that
//! owns it, not here.
//!
//! # Startup, and what it deliberately does not do
//!
//! A restart *reloads and verifies* the durable active ConfigurationRevision.
//! It does not activate whatever the source file now says. Activation is an
//! operator decision with its own durable identity, and a daemon that adopted
//! edited source on restart would make every restart a silent configuration
//! change. The one exception is an installation with nothing active at all,
//! where the first activation is initialization rather than a change — and it
//! is reported as such.
//!
//! A restart also does not rotate the RestoreGeneration. That identifier
//! fences command authority across disaster restore; rotating it on an
//! ordinary restart would invalidate every in-flight operator retry for no
//! reason.

mod fake;
mod options;

use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use pantheon_engine::configuration::{ConfigurationAuthority, ConfigurationStatus, SourceSet};
use pantheon_engine::operator::{OperatorRuntime, ScheduleOutcome};
use pantheon_engine::run::{MinRecoveryPolicy, ReconciliationDeps, RunController};
use pantheon_store::{Command, Store};

use crate::options::{Options, USAGE};

fn main() -> ExitCode {
    let options = match Options::parse(std::env::args().skip(1), |name| std::env::var(name).ok()) {
        Ok(Some(options)) => options,
        Ok(None) => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(err) => {
            eprintln!("pantheond: {err}");
            return ExitCode::FAILURE;
        }
    };

    match run(&options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("pantheond: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run(options: &Options) -> Result<(), String> {
    std::fs::create_dir_all(&options.data_dir)
        .map_err(|err| format!("could not create {}: {err}", options.data_dir.display()))?;

    let store = Arc::new(
        Store::open(options.database())
            .map_err(|err| format!("could not open the authoritative store: {err}"))?,
    );
    let authority = ConfigurationAuthority::new(Arc::clone(&store));
    let status = publish(options, &store, &authority)?;
    report(&status);

    let runtime_store = Arc::clone(&store);
    let runtime = Arc::new(OperatorRuntime::new(store, authority));
    let router = pantheon_operator_api::router(Arc::clone(&runtime));

    let executor = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|err| format!("could not start the runtime: {err}"))?;

    let served = executor.block_on(async {
        // Controllers are supervised here because this is the composition
        // root's job: the engine owns each cycle, the daemon decides when it
        // runs and with which concrete backends. Without --executor fake no
        // backend exists, so every scheduling cycle honestly ends before
        // routing; a later mission supplies real executors.
        let tick = Duration::from_millis(options.tick_millis);
        let backend_choice = if options.fake_executor {
            BackendChoice::Fake
        } else {
            BackendChoice::None
        };
        let scheduler = tokio::spawn(scheduler_loop(Arc::clone(&runtime), backend_choice, tick));
        let runs = if options.fake_executor {
            Some(std::thread::spawn({
                let store = Arc::clone(&runtime_store);
                move || run_controller_thread(store, tick)
            }))
        } else {
            None
        };
        println!(
            "pantheond: operator control on {}",
            options.socket.display()
        );
        let served = pantheon_operator_api::serve(&options.socket, router, shutdown())
            .await
            .map_err(|err| format!("{err}"));
        scheduler.abort();
        drop(runs); // the run-controller thread exits with the process
        served
    });

    served?;
    println!("pantheond: stopped");
    Ok(())
}

/// Ticks the scheduling cycle until shutdown.
///
/// Aborting between ticks is safe by construction: a cycle performs no
/// external effect, so the only thing that can be interrupted is either a
/// read or one authoritative transaction, which commits or rolls back whole.
async fn scheduler_loop(runtime: Arc<OperatorRuntime>, backends: BackendChoice, tick: Duration) {
    loop {
        tokio::time::sleep(tick).await;
        let runtime = Arc::clone(&runtime);
        let outcome = tokio::task::spawn_blocking(move || match backends {
            BackendChoice::None => runtime.service().schedule_once(&[]),
            // The fake executor is the only registered backend; without it
            // routing honestly finds nothing to admit.
            BackendChoice::Fake => {
                let fake = crate::fake::FakeExecutor::new();
                runtime.service().schedule_once(&[fake.port()])
            }
        })
        .await;
        match outcome {
            // Idle and Suppressed are steady states: printing them every tick
            // would be noise, not observability.
            Ok(Ok(ScheduleOutcome::Idle)) | Ok(Ok(ScheduleOutcome::Suppressed(_))) => {}
            Ok(Ok(outcome)) => println!("pantheond: scheduler: {outcome:?}"),
            Ok(Err(err)) => eprintln!("pantheond: scheduler cycle failed: {err}"),
            Err(err) => eprintln!("pantheond: scheduler task failed: {err}"),
        }
    }
}

/// Which concrete backends the composition root registers.
#[derive(Debug, Clone, Copy)]
enum BackendChoice {
    None,
    Fake,
}

/// Runs Run Controller reconciliation on its own thread until shutdown.
///
/// A dedicated OS thread rather than a spawned task: the controller object
/// must persist across ticks so bearer memory survives between passes (a
/// fresh controller per tick would rekey before every launch), and its
/// transactions belong on blocking threads anyway. Losing the object is what
/// a crash does, and recovery is the tested restart path.
fn run_controller_thread(store: Arc<Store>, tick: Duration) {
    // The minimum deterministic recovery policy and the fake Sandbox gate
    // are composition facts of this build, not per-tick decisions.
    let policy = MinRecoveryPolicy::default();
    let mut controller = RunController::new(
        &store,
        pantheon_engine::run::OsRandom,
        format!("pantheond-{}", std::process::id()),
    );
    loop {
        std::thread::sleep(tick);
        let fake = crate::fake::FakeExecutor::new();
        let deps = ReconciliationDeps {
            launcher: &fake,
            sandbox: &fake,
            policy: &policy,
        };
        match controller.reconcile_all(&deps) {
            Ok(results) if results.is_empty() => {}
            Ok(results) => {
                for (run_id, result) in results {
                    match result {
                        Ok(outcome) => println!("pantheond: run-controller: {run_id}: {outcome:?}"),
                        Err(err) => {
                            eprintln!("pantheond: run-controller: {run_id}: failed: {err}")
                        }
                    }
                }
            }
            Err(err) => eprintln!("pantheond: run controller inventory failed: {err}"),
        }
    }
}

/// Establishes the configuration authority this daemon will serve under.
///
/// See the module documentation for why a restart verifies rather than
/// adopts.
fn publish(
    options: &Options,
    store: &Store,
    authority: &ConfigurationAuthority<Arc<Store>>,
) -> Result<ConfigurationStatus, String> {
    let text = std::fs::read_to_string(&options.configuration).map_err(|err| {
        format!(
            "could not read the configuration source {}: {err}",
            options.configuration.display()
        )
    })?;
    let name = options.configuration.file_name().map_or_else(
        || "configuration".to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    let sources = SourceSet::single(name, text);

    let loaded = authority
        .load(&sources)
        .map_err(|err| format!("could not load durable configuration: {err}"))?;
    if !matches!(loaded, ConfigurationStatus::Uninitialized) {
        return Ok(loaded);
    }

    // Nothing is active. This is a first start, not a restart, so activating
    // is initialization rather than adopting an edit.
    let epoch = store
        .restore_generation()
        .map_err(|err| format!("could not read the installation's command epoch: {err}"))?;
    // The command id is derived from the source identity, so a crash between
    // activation and the reload below leaves the next start replaying the
    // same command rather than activating a second time.
    let digest = sources.digest();
    let command_id = format!("bootstrap-{}", digest.to_hex());
    authority
        .activate(
            &Command {
                epoch: epoch.as_str(),
                id: &command_id,
                request_hash: digest.as_bytes(),
                event_type: "configuration.activated",
            },
            &sources,
        )
        .map_err(|err| format!("could not activate the initial configuration: {err}"))?;
    authority
        .load(&sources)
        .map_err(|err| format!("could not load the configuration just activated: {err}"))
}

fn report(status: &ConfigurationStatus) {
    match status {
        ConfigurationStatus::Uninitialized => {
            println!("pantheond: no active configuration; the daemon is not ready");
        }
        ConfigurationStatus::Active { active } => println!(
            "pantheond: configuration revision {} active",
            active.activation_sequence
        ),
        ConfigurationStatus::Drifted { active, .. } => println!(
            "pantheond: configuration revision {} active, source has drifted and was not adopted",
            active.activation_sequence
        ),
    }
}

/// Resolves on the first interrupt or termination signal.
///
/// Graceful rather than abrupt so an authoritative transaction in flight
/// finishes or rolls back on its own terms.
async fn shutdown() {
    let interrupt = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    tokio::select! {
        () = interrupt => {}
        () = terminate => {}
    }
    println!("pantheond: shutting down");
}
