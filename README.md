# liner

An open-source music metadata server. liner reads artist, release group, and label data from a MusicBrainz mirror, adds bios and images from other sources, and builds a similar-artists graph with an LLM. See [PLAN.md](PLAN.md) for the design and build order.

Status: the MusicBrainz endpoints, enrichment, the job queue, and the similar-artists graph work.

## Run locally

```bash
docker compose up -d postgres redis
cargo run            # liner serve, on :3002
cargo run -- worker  # background jobs
cargo test
```

## Configuration

| Variable | Default |
| --- | --- |
| `PORT` | `3002` |
| `DATABASE_URL` | `postgres://liner:liner@localhost:5433/liner` |
| `REDIS_URL` | `redis://localhost:6380` |
| `MUSICBRAINZ_API_URL` | `https://mb.halfnotes.club` |
| `ADMIN_TOKEN` | unset |
| `TRUST_PROXY` | `true` |
| `RATE_LIMIT_ANON_PER_MIN` | `60` |
| `RATE_LIMIT_KEY_PER_MIN` | `1200` |
| `LASTFM_API_KEY`, `FANART_API_KEY` | unset. Last.fm adds bios, tags and similar artists; fanart.tv adds artist images |
| `LLM_API_KEY` | unset. Without a key, `/v1/artist/{mbid}/similar` returns unranked candidates |
| `LLM_BASE_URL`, `LLM_MODEL`, `LLM_DAILY_ARTIST_CAP` | `https://api.openai.com/v1`, `gpt-6.1-sol`, `200` |
| `ENRICH_JOB_INTERVAL_MS` | `1000`. Least time between two bio and image jobs, to go easy on Wikidata and Wikipedia |
| `WORKER_METRICS_PORT` | `9090`. The worker serves `/metrics` and `/healthz` here |

## The job queue

`GET /v1/artist/{mbid}` answers from what is stored and never waits on Wikidata, Wikipedia, Last.fm, or fanart.tv. If nothing recent is stored it queues an `enrich` job and sets `enrichment: "pending"`; the worker fetches the sources and stores bios, tags, and images, and the next request returns them with `enrichment: "complete"`. Similar-artist ranking runs through the same queue.

A source that answers `429` pauses that kind of job for its `Retry-After` (30 seconds to 15 minutes, 60 by default). The paused job keeps its attempts; other failures retry twice more with a growing delay, then stay as `failed`. Finished jobs are pruned after a week.

Metrics, on the API's `/metrics` (queue gauges) and the worker's (counters):

- `liner_queue_jobs{kind,status}` and `liner_queue_oldest_due_seconds{kind}`
- `liner_jobs_total{kind,outcome}` with outcome `done`, `retry`, `failed`, or `deferred`, and `liner_job_duration_seconds{kind}`
- `liner_source_requests_total{source,outcome}` and `liner_llm_requests_total{outcome}`

`infra/monitoring.ts` adds the ServiceMonitors, a `liner` dashboard in Grafana, and alert rules to the halfnotes monitoring stack.

## Rate limits and API keys

Anonymous clients get `RATE_LIMIT_ANON_PER_MIN` requests per minute per IP. Clients that send a valid `X-Liner-Key` header get `RATE_LIMIT_KEY_PER_MIN`, or the key's own limit. Over the limit, liner returns `429` with `Retry-After`.

```bash
cargo run -- key create mu 2000   # prints the key once
```

## Deploy

CI builds the image and runs `pulumi up` in `infra/` on every push to `main`. The stack deploys onto the halfnotes `dev` cluster. It expects a `liner` role and database in the in-cluster Postgres, and the ingress, cert-manager, and `letsencrypt` issuer from the halfnotes stack.

## License

MIT
