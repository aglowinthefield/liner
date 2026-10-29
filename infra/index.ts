// Deploys liner onto the existing halfnotes `dev` cluster. Run `doctl kubernetes cluster
// kubeconfig save <cluster>` first; the Kubernetes provider uses the ambient kubeconfig.
//
// Prerequisites that live outside this stack:
//   - a `liner` role and database in the in-cluster Postgres (halfnotes-db)
//   - the nginx ingress, cert-manager and `letsencrypt` ClusterIssuer from the halfnotes stack
import * as pulumi from "@pulumi/pulumi";
import * as k8s from "@pulumi/kubernetes";
import * as cloudflare from "@pulumi/cloudflare";
import { monitoring } from "./monitoring";

const config = new pulumi.Config();
const image = config.get("image") || "registry.digitalocean.com/halfnotes/liner:latest";
const host = config.get("host") || "liner.halfnotes.club";
const ingressIp = config.require("ingressIp");
const zoneId = config.require("cloudflareZoneId");

const labels = (app: string) => ({ app });

const secret = new k8s.core.v1.Secret("liner", {
  metadata: { name: "liner" },
  stringData: pulumi
    .all([
      config.requireSecret("databasePassword"),
      config.requireSecret("adminToken"),
      config.getSecret("lastfmApiKey"),
      config.getSecret("fanartApiKey"),
      config.getSecret("llmApiKey"),
    ])
    .apply(([dbPassword, adminToken, lastfm, fanart, llm]) => {
      const data: Record<string, string> = {
        DATABASE_URL: `postgres://liner:${dbPassword}@halfnotes-db:5432/liner`,
        ADMIN_TOKEN: adminToken,
      };
      if (lastfm) data.LASTFM_API_KEY = lastfm;
      if (fanart) data.FANART_API_KEY = fanart;
      if (llm) data.LLM_API_KEY = llm;
      return data;
    }),
});

const env: k8s.types.input.core.v1.EnvVar[] = [
  { name: "PORT", value: "3002" },
  { name: "REDIS_URL", value: "redis://halfnotes-redis:6379" },
  { name: "MUSICBRAINZ_API_URL", value: "https://mb.halfnotes.club" },
  { name: "TRUST_PROXY", value: "true" },
  { name: "LLM_MODEL", value: config.get("llmModel") || "gpt-6.1-sol" },
  { name: "LLM_DAILY_ARTIST_CAP", value: config.get("llmDailyArtistCap") || "200" },
  // Least time between two enrich jobs, so Wikidata and Wikipedia see a steady trickle.
  { name: "ENRICH_JOB_INTERVAL_MS", value: config.get("enrichJobIntervalMs") || "1000" },
  { name: "WORKER_METRICS_PORT", value: "9090" },
];

function deployment(name: string, args: string[], port?: number, portName?: string) {
  const l = labels(name);
  return new k8s.apps.v1.Deployment(name, {
    metadata: { name },
    spec: {
      replicas: 1,
      selector: { matchLabels: l },
      template: {
        metadata: { labels: l },
        spec: {
          containers: [{
            name,
            image,
            imagePullPolicy: "Always",
            command: ["liner", ...args],
            envFrom: [{ secretRef: { name: secret.metadata.name } }],
            env,
            ports: port ? [{ name: portName, containerPort: port }] : undefined,
            readinessProbe: port
              ? { httpGet: { path: "/healthz", port }, periodSeconds: 10 }
              : undefined,
            resources: {
              requests: { cpu: "50m", memory: "64Mi" },
              limits: { memory: "256Mi" },
            },
          }],
        },
      },
    },
  });
}

export const api = deployment("liner-api", ["serve"], 3002);
const workerMetricsPort = 9090;
export const worker = deployment("liner-worker", ["worker"], workerMetricsPort, "metrics");
export const monitors = monitoring(workerMetricsPort);

export const service = new k8s.core.v1.Service("liner-api", {
  metadata: { name: "liner-api" },
  spec: {
    type: "ClusterIP",
    selector: labels("liner-api"),
    ports: [{ name: "http", port: 3002, targetPort: 3002 }],
  },
});

export const ingress = new k8s.networking.v1.Ingress("liner", {
  metadata: {
    name: "liner",
    annotations: { "cert-manager.io/cluster-issuer": "letsencrypt" },
  },
  spec: {
    ingressClassName: "nginx",
    tls: [{ hosts: [host], secretName: "liner-tls" }],
    rules: [{
      host,
      http: {
        paths: [{
          path: "/",
          pathType: "Prefix",
          backend: { service: { name: "liner-api", port: { number: 3002 } } },
        }],
      },
    }],
  },
});

export const dns = new cloudflare.Record("liner", {
  zoneId,
  name: "liner",
  type: "A",
  content: ingressIp,
  proxied: true,
});

export const url = `https://${host}`;
