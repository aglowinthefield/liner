use axum::response::IntoResponse;
use axum_prometheus::{PrometheusMetricLayer, AXUM_HTTP_REQUESTS_DURATION_SECONDS};
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use std::sync::OnceLock;

static PROM_HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();

fn handle() -> &'static PrometheusHandle {
    PROM_HANDLE.get_or_init(|| {
        PrometheusBuilder::new()
            .set_buckets_for_metric(
                Matcher::Full(AXUM_HTTP_REQUESTS_DURATION_SECONDS.to_string()),
                &[0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0],
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

pub async fn metrics_handler() -> impl IntoResponse {
    handle().render()
}
