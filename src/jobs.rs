//! A small job queue in Postgres. Jobs are claimed with a lease, so a worker that dies
//! mid-job leaves the job to be picked up again after the lease runs out.

use sqlx::PgPool;
use uuid::Uuid;

pub const KIND_SIMILAR: &str = "similar";
pub const KIND_ENRICH: &str = "enrich";
pub const KINDS: [&str; 2] = [KIND_SIMILAR, KIND_ENRICH];
pub const STATUSES: [&str; 4] = ["pending", "running", "done", "failed"];

pub const MAX_ATTEMPTS: i32 = 3;
const LEASE_MINUTES: i32 = 10;

#[derive(Debug, sqlx::FromRow)]
pub struct Job {
    pub id: i64,
    pub kind: String,
    pub mbid: Uuid,
    pub attempts: i32,
}

/// Queues a job unless the same job is already pending or running. Returns true if it was queued.
pub async fn enqueue(pool: &PgPool, kind: &str, mbid: Uuid) -> anyhow::Result<bool> {
    let queued = sqlx::query(
        "INSERT INTO jobs (kind, mbid) VALUES ($1, $2)
         ON CONFLICT (kind, mbid) WHERE status IN ('pending', 'running') DO NOTHING",
    )
    .bind(kind)
    .bind(mbid)
    .execute(pool)
    .await?;
    Ok(queued.rows_affected() > 0)
}

/// Takes the oldest due job whose kind is not in `skip`, or None if there is nothing to do.
pub async fn claim(pool: &PgPool, skip: &[&str]) -> anyhow::Result<Option<Job>> {
    Ok(sqlx::query_as::<_, Job>(
        "UPDATE jobs SET status = 'running', attempts = attempts + 1, updated_at = now(),
                run_after = now() + make_interval(mins => $1)
         WHERE id = (
             SELECT id FROM jobs
             WHERE status IN ('pending', 'running') AND run_after <= now()
               AND kind <> ALL($2)
             ORDER BY run_after
             FOR UPDATE SKIP LOCKED
             LIMIT 1)
         RETURNING id, kind, mbid, attempts",
    )
    .bind(LEASE_MINUTES)
    .bind(skip)
    .fetch_optional(pool)
    .await?)
}

pub async fn complete(pool: &PgPool, job: &Job) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE jobs SET status = 'done', last_error = NULL, updated_at = now() WHERE id = $1",
    )
    .bind(job.id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Puts the job back with a growing delay, or marks it failed after the last attempt.
pub async fn fail(pool: &PgPool, job: &Job, error: &str) -> anyhow::Result<()> {
    if job.attempts >= MAX_ATTEMPTS {
        sqlx::query(
            "UPDATE jobs SET status = 'failed', last_error = $2, updated_at = now() WHERE id = $1",
        )
        .bind(job.id)
        .bind(error)
        .execute(pool)
        .await?;
    } else {
        sqlx::query(
            "UPDATE jobs SET status = 'pending', last_error = $2, updated_at = now(),
                    run_after = now() + make_interval(mins => $3)
             WHERE id = $1",
        )
        .bind(job.id)
        .bind(error)
        .bind(job.attempts * 2)
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// Status of the newest job of this kind for the artist, if any.
pub async fn latest_status(
    pool: &PgPool,
    kind: &str,
    mbid: Uuid,
) -> anyhow::Result<Option<String>> {
    Ok(sqlx::query_scalar(
        "SELECT status FROM jobs WHERE kind = $1 AND mbid = $2 ORDER BY id DESC LIMIT 1",
    )
    .bind(kind)
    .bind(mbid)
    .fetch_optional(pool)
    .await?)
}

/// Puts the job back without using up an attempt, to run again after `delay`.
pub async fn defer(
    pool: &PgPool,
    job: &Job,
    delay: std::time::Duration,
    reason: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE jobs SET status = 'pending', attempts = GREATEST(attempts - 1, 0), last_error = $2,
                updated_at = now(), run_after = now() + make_interval(secs => $3)
         WHERE id = $1",
    )
    .bind(job.id)
    .bind(reason)
    .bind(delay.as_secs_f64())
    .execute(pool)
    .await?;
    Ok(())
}

/// Deletes finished jobs older than `days`. Returns how many went.
pub async fn prune(pool: &PgPool, days: i32) -> anyhow::Result<u64> {
    Ok(sqlx::query(
        "DELETE FROM jobs WHERE status = 'done' AND updated_at < now() - make_interval(days => $1)",
    )
    .bind(days)
    .execute(pool)
    .await?
    .rows_affected())
}

#[derive(Debug, sqlx::FromRow)]
pub struct QueueStat {
    pub kind: String,
    pub status: String,
    pub count: i64,
    /// For pending jobs: how long the oldest one has been due.
    pub oldest_due_secs: Option<f64>,
}

pub async fn stats(pool: &PgPool) -> anyhow::Result<Vec<QueueStat>> {
    Ok(sqlx::query_as(
        "SELECT kind, status, count(*) AS count,
                max(EXTRACT(EPOCH FROM now() - run_after))::float8 AS oldest_due_secs
         FROM jobs GROUP BY kind, status",
    )
    .fetch_all(pool)
    .await?)
}

/// Newest job of this kind for the artist, with its age in seconds.
pub async fn latest(
    pool: &PgPool,
    kind: &str,
    mbid: Uuid,
) -> anyhow::Result<Option<(String, f64)>> {
    Ok(sqlx::query_as(
        "SELECT status, EXTRACT(EPOCH FROM now() - updated_at)::float8
         FROM jobs WHERE kind = $1 AND mbid = $2 ORDER BY id DESC LIMIT 1",
    )
    .bind(kind)
    .bind(mbid)
    .fetch_optional(pool)
    .await?)
}
