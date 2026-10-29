use std::time::Duration;

use crate::{jobs, similar, AppState};

const JOB_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const IDLE_POLL: Duration = Duration::from_secs(2);

/// Runs jobs one at a time, forever.
pub async fn run(state: AppState) {
    tracing::info!("liner worker started");
    loop {
        match run_one(&state).await {
            Ok(true) => {}
            Ok(false) => tokio::time::sleep(IDLE_POLL).await,
            Err(e) => {
                tracing::error!("claiming a job failed: {e}");
                tokio::time::sleep(IDLE_POLL * 5).await;
            }
        }
    }
}

/// Claims and runs one due job. Returns false if there was none.
pub async fn run_one(state: &AppState) -> anyhow::Result<bool> {
    let Some(job) = jobs::claim(&state.pool).await? else {
        return Ok(false);
    };
    let outcome = match tokio::time::timeout(JOB_TIMEOUT, process(state, &job)).await {
        Ok(r) => r,
        Err(_) => Err(anyhow::anyhow!(
            "timed out after {}s",
            JOB_TIMEOUT.as_secs()
        )),
    };
    let recorded = match outcome {
        Ok(()) => jobs::complete(&state.pool, &job).await,
        Err(e) => {
            tracing::warn!(
                "job {} ({} {}) failed on attempt {}: {e:#}",
                job.id,
                job.kind,
                job.mbid,
                job.attempts
            );
            jobs::fail(&state.pool, &job, &format!("{e:#}")).await
        }
    };
    if let Err(e) = recorded {
        tracing::error!("recording the result of job {} failed: {e}", job.id);
    }
    Ok(true)
}

async fn process(state: &AppState, job: &jobs::Job) -> anyhow::Result<()> {
    match job.kind.as_str() {
        jobs::KIND_SIMILAR => similar::run_job(state, job.mbid).await,
        other => anyhow::bail!("unknown job kind `{other}`"),
    }
}
