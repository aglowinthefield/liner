// Prometheus scraping, a Grafana dashboard, and alerts for liner. They plug into the
// kube-prometheus-stack that the halfnotes stack runs in the `monitoring` namespace:
// Prometheus picks up any ServiceMonitor, and Grafana's sidecar loads ConfigMaps labelled
// `grafana_dashboard`.
import * as k8s from "@pulumi/kubernetes";
import { linerDashboard } from "./dashboard";

const release = { release: "kube-prometheus-stack" };
// This Prometheus only loads rules labelled with its Helm release name.
const ruleRelease = { release: "kube-prometheus-stack-ce6e0f4e" };

export function monitoring(workerMetricsPort: number) {
  const apiMonitor = new k8s.apiextensions.CustomResource("liner-api-monitor", {
    apiVersion: "monitoring.coreos.com/v1",
    kind: "ServiceMonitor",
    metadata: { name: "liner-api", namespace: "default", labels: release },
    spec: {
      selector: { matchLabels: { app: "liner-api" } },
      endpoints: [{ port: "http", path: "/metrics", interval: "15s" }],
    },
  });

  const workerMonitor = new k8s.apiextensions.CustomResource("liner-worker-monitor", {
    apiVersion: "monitoring.coreos.com/v1",
    kind: "ServiceMonitor",
    metadata: { name: "liner-worker", namespace: "default", labels: release },
    spec: {
      selector: { matchLabels: { app: "liner-worker" } },
      endpoints: [{ port: "metrics", path: "/metrics", interval: "15s" }],
    },
  });

  const dashboard = new k8s.core.v1.ConfigMap("liner-dashboard", {
    metadata: {
      name: "liner-dashboard",
      namespace: "monitoring",
      labels: { grafana_dashboard: "1" },
    },
    data: { "liner.json": JSON.stringify(linerDashboard()) },
  });

  const alerts = new k8s.apiextensions.CustomResource("liner-alerts", {
    apiVersion: "monitoring.coreos.com/v1",
    kind: "PrometheusRule",
    metadata: { name: "liner-alerts", namespace: "monitoring", labels: ruleRelease },
    spec: {
      groups: [{
        name: "liner",
        rules: [
          {
            alert: "LinerApiDown",
            expr: 'sum(up{job="liner-api"}) == 0 or absent(up{job="liner-api"})',
            for: "2m",
            labels: { severity: "critical" },
            annotations: { summary: "liner API is down" },
          },
          {
            alert: "LinerWorkerDown",
            expr: 'sum(up{job="liner-worker"}) == 0 or absent(up{job="liner-worker"})',
            for: "5m",
            labels: { severity: "critical" },
            annotations: { summary: "liner worker is down, so the queue is not draining" },
          },
          {
            alert: "LinerQueueStuck",
            expr: 'max by (kind) (liner_queue_oldest_due_seconds) > 900',
            for: "5m",
            labels: { severity: "warning" },
            annotations: {
              summary: "liner {{ $labels.kind }} jobs have waited over 15 minutes",
              description: "The worker is down, paused by a rate limit, or slower than the intake.",
            },
          },
          {
            alert: "LinerJobsFailing",
            expr: 'sum by (kind) (increase(liner_jobs_total{outcome="failed"}[30m])) > 5',
            labels: { severity: "warning" },
            annotations: { summary: "liner {{ $labels.kind }} jobs ran out of retries" },
          },
          {
            alert: "LinerSourceRateLimited",
            expr: 'sum by (source) (increase(liner_source_requests_total{outcome="rate_limited"}[15m])) > 0',
            for: "15m",
            labels: { severity: "warning" },
            annotations: {
              summary: "{{ $labels.source }} keeps rate limiting liner",
              description: "Slow the worker down with ENRICH_JOB_INTERVAL_MS.",
            },
          },
          {
            alert: "LinerApiHighErrorRate",
            expr: 'sum(rate(axum_http_requests_total{job="liner-api",status=~"5.."}[5m])) / sum(rate(axum_http_requests_total{job="liner-api"}[5m])) > 0.05',
            for: "5m",
            labels: { severity: "warning" },
            annotations: { summary: "More than 5% of liner API requests return 5xx" },
          },
        ],
      }],
    },
  });

  const workerService = new k8s.core.v1.Service("liner-worker", {
    metadata: { name: "liner-worker", labels: { app: "liner-worker" } },
    spec: {
      type: "ClusterIP",
      selector: { app: "liner-worker" },
      ports: [{ name: "metrics", port: workerMetricsPort, targetPort: workerMetricsPort }],
    },
  });

  return { apiMonitor, workerMonitor, dashboard, alerts, workerService };
}
