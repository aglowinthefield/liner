use std::time::{Duration, Instant};

use crate::{enrich, jobs, similar, sources::RateLimited, AppState};

const JOB_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const IDLE_POLL: Duration = Duration::from_secs(2);
const PRUNE_EVERY: Duration = Duration::from_secs(10 * 60);
const KEEP_DONE_DAYS: i32 = 7;
/// How long to pause a kind of job after a rate limit that gave no `Retry-After`.
const DEFAULT_COOLDOWN: Duration = Duration::from_secs(60);
const MIN_COOLDOWN: Duration = Duration::from_secs(30);
const MAX_COOLDOWN: Duration = Duration::from_secs(15 * 60);

#[derive(Clone, Copy)]
pub struct WorkerConfig {
    /// Least time between two `enrich` jobs, so Wikidata and Wikipedia see a steady trickle.
    pub enrich_interval: Duration,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            enrich_interval: Duration::from_millis(1000),
        }
    }
}

fn cooldown_key(kind: &str) -> String {
    format!("liner:cooldown:{kind}")
}

/// Runs jobs one at a time, forever.
pub async fn run(state: AppState, config: WorkerConfig) {
    tracing::info!("liner worker started");
    let mut last_enrich: Option<Instant> = None;
    let mut last_prune = Instant::now();
    loop {
        let throttled = last_enrich.is_some_and(|t| t.elapsed() < config.enrich_interval);
        let skip: &[&str] = if throttled { &[jobs::KIND_ENRICH] } else { &[] };
        match run_once(&state, skip).await {
            Ok(Some(kind)) => {
                if kind == jobs::KIND_ENRICH {
                    last_enrich = Some(Instant::now());
                }
            }
            Ok(None) => {
                if last_prune.elapsed() > PRUNE_EVERY {
                    last_prune = Instant::now();
                    match jobs::prune(&state.pool, KEEP_DONE_DAYS).await {
                        Ok(n) if n > 0 => tracing::info!("pruned {n} finished jobs"),
                        Ok(_) => {}
                        Err(e) => tracing::warn!("pruning jobs failed: {e}"),
                    }
                }
                tokio::time::sleep(if throttled {
                    Duration::from_millis(100)
                } else {
                    IDLE_POLL
                })
                .await
            }
            Err(e) => {
                tracing::error!("claiming a job failed: {e}");
                tokio::time::sleep(IDLE_POLL * 5).await;
            }
        }
    }
}

/// Claims and runs one due job. Returns false if there was none.
pub async fn run_one(state: &AppState) -> anyhow::Result<bool> {
    Ok(run_once(state, &[]).await?.is_some())
}

/// Like [`run_one`], skipping the kinds in `skip`. Returns the kind that ran.
async fn run_once(state: &AppState, skip: &[&str]) -> anyhow::Result<Option<String>> {
    // Kinds paused by a rate limit are left in the queue.
    let mut skip: Vec<&str> = skip.to_vec();
    for kind in jobs::KINDS {
        if !skip.contains(&kind) && state.cache.is_marked(&cooldown_key(kind)).await {
            skip.push(kind);
        }
    }
    let Some(job) = jobs::claim(&state.pool, &skip).await? else {
        return Ok(None);
    };
    let started = Instant::now();
    let outcome = match tokio::time::timeout(JOB_TIMEOUT, process(state, &job)).await {
        Ok(r) => r,
        Err(_) => Err(anyhow::anyhow!(
            "timed out after {}s",
            JOB_TIMEOUT.as_secs()
        )),
    };
    let kind: &'static str = jobs::KINDS
        .into_iter()
        .find(|k| *k == job.kind)
        .unwrap_or("other");
    metrics::histogram!("liner_job_duration_seconds", "kind" => kind)
        .record(started.elapsed().as_secs_f64());
    let (label, recorded) = match outcome {
        Ok(()) => ("done", jobs::complete(&state.pool, &job).await),
        Err(e) => match e.downcast_ref::<RateLimited>() {
            Some(limited) => {
                let pause = limited
                    .retry_after
                    .unwrap_or(DEFAULT_COOLDOWN)
                    .clamp(MIN_COOLDOWN, MAX_COOLDOWN);
                tracing::warn!(
                    "job {} ({} {}) hit a rate limit, pausing {} jobs for {}s: {e}",
                    job.id,
                    job.kind,
                    job.mbid,
                    job.kind,
                    pause.as_secs()
                );
                state.cache.mark(&cooldown_key(&job.kind), pause).await;
                (
                    "deferred",
                    jobs::defer(&state.pool, &job, pause, &format!("{e:#}")).await,
                )
            }
            None => {
                tracing::warn!(
                    "job {} ({} {}) failed on attempt {}: {e:#}",
                    job.id,
                    job.kind,
                    job.mbid,
                    job.attempts
                );
                let label = if job.attempts >= jobs::MAX_ATTEMPTS {
                    "failed"
                } else {
                    "retry"
                };
                (
                    label,
                    jobs::fail(&state.pool, &job, &format!("{e:#}")).await,
                )
            }
        },
    };
    metrics::counter!("liner_jobs_total", "kind" => kind, "outcome" => label).increment(1);
    if let Err(e) = recorded {
        tracing::error!("recording the result of job {} failed: {e}", job.id);
    }
    Ok(Some(job.kind))
}

async fn process(state: &AppState, job: &jobs::Job) -> anyhow::Result<()> {
    match job.kind.as_str() {
        jobs::KIND_SIMILAR => similar::run_job(state, job.mbid).await,
        jobs::KIND_ENRICH => enrich::run_job(state, job.mbid).await,
        other => anyhow::bail!("unknown job kind `{other}`"),
    }
}
