// The liner Grafana dashboard, built in code so panels stay one line each.

const ds = { type: "prometheus", uid: "prometheus" };

type Unit = "short" | "s" | "reqps" | "percentunit" | "ops";

let nextId = 1;

function panel(
  title: string,
  x: number,
  y: number,
  w: number,
  type: "timeseries" | "stat",
  unit: Unit,
  targets: { expr: string; legend: string }[],
  extra: Record<string, unknown> = {},
) {
  return {
    id: nextId++,
    title,
    type,
    datasource: ds,
    gridPos: { x, y, w, h: 8 },
    fieldConfig: { defaults: { unit }, overrides: [] },
    targets: targets.map((t, i) => ({
      refId: String.fromCharCode(65 + i),
      datasource: ds,
      expr: t.expr,
      legendFormat: t.legend,
    })),
    ...extra,
  };
}

function row(title: string, y: number) {
  return { id: nextId++, title, type: "row", collapsed: false, gridPos: { x: 0, y, w: 24, h: 1 }, panels: [] };
}

export function linerDashboard() {
  nextId = 1;
  const panels = [
    row("Queue", 0),
    panel("Pending jobs", 0, 1, 6, "stat", "short", [
      { expr: 'max by (kind) (liner_queue_jobs{status="pending"})', legend: "{{kind}}" },
    ]),
    panel("Oldest due job", 6, 1, 6, "stat", "s", [
      { expr: "max by (kind) (liner_queue_oldest_due_seconds)", legend: "{{kind}}" },
    ]),
    panel("Queue depth", 12, 1, 12, "timeseries", "short", [
      { expr: 'max by (kind, status) (liner_queue_jobs{status=~"pending|running|failed"})', legend: "{{kind}} {{status}}" },
    ]),
    panel("Jobs finished by outcome", 0, 9, 12, "timeseries", "ops", [
      { expr: "sum by (kind, outcome) (rate(liner_jobs_total[5m]))", legend: "{{kind}} {{outcome}}" },
    ]),
    panel("Job duration p50 / p95", 12, 9, 12, "timeseries", "s", [
      { expr: "histogram_quantile(0.5, sum by (le, kind) (rate(liner_job_duration_seconds_bucket[10m])))", legend: "p50 {{kind}}" },
      { expr: "histogram_quantile(0.95, sum by (le, kind) (rate(liner_job_duration_seconds_bucket[10m])))", legend: "p95 {{kind}}" },
    ]),
    row("Outside sources", 17),
    panel("Requests by outcome", 0, 18, 12, "timeseries", "ops", [
      { expr: "sum by (source, outcome) (rate(liner_source_requests_total[5m]))", legend: "{{source}} {{outcome}}" },
    ]),
    panel("Rate limited (last hour)", 12, 18, 6, "stat", "short", [
      { expr: 'sum by (source) (increase(liner_source_requests_total{outcome="rate_limited"}[1h]))', legend: "{{source}}" },
    ]),
    panel("LLM calls (last 24h)", 18, 18, 6, "stat", "short", [
      { expr: "sum by (outcome) (increase(liner_llm_requests_total[24h]))", legend: "{{outcome}}" },
    ]),
    row("API", 26),
    panel("Requests per second", 0, 27, 8, "timeseries", "reqps", [
      { expr: 'sum by (status) (rate(axum_http_requests_total{job="liner-api"}[5m]))', legend: "{{status}}" },
    ]),
    panel("Latency p50 / p95", 8, 27, 8, "timeseries", "s", [
      { expr: 'histogram_quantile(0.5, sum by (le) (rate(axum_http_requests_duration_seconds_bucket{job="liner-api"}[5m])))', legend: "p50" },
      { expr: 'histogram_quantile(0.95, sum by (le) (rate(axum_http_requests_duration_seconds_bucket{job="liner-api"}[5m])))', legend: "p95" },
    ]),
    panel("5xx rate", 16, 27, 8, "timeseries", "percentunit", [
      {
        expr: 'sum(rate(axum_http_requests_total{job="liner-api",status=~"5.."}[5m])) / sum(rate(axum_http_requests_total{job="liner-api"}[5m]))',
        legend: "5xx",
      },
    ]),
    panel("Requests by endpoint", 0, 35, 12, "timeseries", "reqps", [
      { expr: 'sum by (endpoint) (rate(axum_http_requests_total{job="liner-api"}[5m]))', legend: "{{endpoint}}" },
    ]),
    panel("Targets up", 12, 35, 12, "stat", "short", [
      { expr: 'up{job=~"liner-api|liner-worker"}', legend: "{{job}}" },
    ]),
  ];
  return {
    uid: "liner",
    title: "liner",
    tags: ["liner"],
    timezone: "browser",
    schemaVersion: 39,
    refresh: "30s",
    time: { from: "now-6h", to: "now" },
    panels,
  };
}
