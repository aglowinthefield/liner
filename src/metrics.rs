use axum::{extract::State, response::IntoResponse};
use axum_prometheus::{PrometheusMetricLayer, AXUM_HTTP_REQUESTS_DURATION_SECONDS};
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use std::sync::OnceLock;

use crate::{jobs, AppState};

static PROM_HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();

fn handle() -> &'static PrometheusHandle {
    PROM_HANDLE.get_or_init(|| {
        PrometheusBuilder::new()
            .set_buckets_for_metric(
                Matcher::Full(AXUM_HTTP_REQUESTS_DURATION_SECONDS.to_string()),
                &[
                    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
                ],
            )
            .unwrap()
            .set_buckets_for_metric(
                Matcher::Full("liner_job_duration_seconds".to_string()),
                &[0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0, 120.0],
            )
            .unwrap()
            .install_recorder()
            .unwrap()
    })
}

pub fn layer() -> PrometheusMetricLayer<'static> {
    let _ = handle(); // ensure recorder is installed
    PrometheusMetricLayer::new()
}

/// Installs the recorder without the HTTP layer. For the worker, which has no routes.
pub fn install() {
    let _ = handle();
}

/// Sets the queue gauges from the jobs table. Only the API does this at scrape time, so
/// Prometheus sees one set of values for the queue.
pub async fn update_queue_gauges(state: &AppState) {
    let stats = match jobs::stats(&state.pool).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("reading queue stats failed: {e}");
            return;
        }
    };
    for kind in jobs::KINDS {
        let mut oldest = 0.0_f64;
        for status in jobs::STATUSES {
            let found = stats.iter().find(|s| s.kind == kind && s.status == status);
            metrics::gauge!("liner_queue_jobs", "kind" => kind, "status" => status)
                .set(found.map_or(0.0, |s| s.count as f64));
            if status == "pending" {
                oldest = found
                    .and_then(|s| s.oldest_due_secs)
                    .unwrap_or(0.0)
                    .max(0.0);
            }
        }
        metrics::gauge!("liner_queue_oldest_due_seconds", "kind" => kind).set(oldest);
    }
}

pub async fn metrics_handler(State(state): State<AppState>) -> impl IntoResponse {
    update_queue_gauges(&state).await;
    handle().render()
}

/// The worker's `/metrics`: counters and histograms only.
pub async fn worker_metrics_handler() -> impl IntoResponse {
    handle().render()
}
