//! A small job queue in Postgres. Jobs are claimed with a lease, so a worker that dies
//! mid-job leaves the job to be picked up again after the lease runs out.

use sqlx::PgPool;
use uuid::Uuid;

pub const KIND_SIMILAR: &str = "similar";

const MAX_ATTEMPTS: i32 = 3;
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

/// Takes the oldest due job, or None if there is nothing to do.
pub async fn claim(pool: &PgPool) -> anyhow::Result<Option<Job>> {
    Ok(sqlx::query_as::<_, Job>(
        "UPDATE jobs SET status = 'running', attempts = attempts + 1,
                run_after = now() + make_interval(mins => $1)
         WHERE id = (
             SELECT id FROM jobs
             WHERE status IN ('pending', 'running') AND run_after <= now()
             ORDER BY run_after
             FOR UPDATE SKIP LOCKED
             LIMIT 1)
         RETURNING id, kind, mbid, attempts",
    )
    .bind(LEASE_MINUTES)
    .fetch_optional(pool)
    .await?)
}

pub async fn complete(pool: &PgPool, job: &Job) -> anyhow::Result<()> {
    sqlx::query("UPDATE jobs SET status = 'done', last_error = NULL WHERE id = $1")
        .bind(job.id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Puts the job back with a growing delay, or marks it failed after the last attempt.
pub async fn fail(pool: &PgPool, job: &Job, error: &str) -> anyhow::Result<()> {
    if job.attempts >= MAX_ATTEMPTS {
        sqlx::query("UPDATE jobs SET status = 'failed', last_error = $2 WHERE id = $1")
            .bind(job.id)
            .bind(error)
            .execute(pool)
            .await?;
    } else {
        sqlx::query(
            "UPDATE jobs SET status = 'pending', last_error = $2, run_after = now() + make_interval(mins => $3)
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
