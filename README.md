# liner

An open-source music metadata server. liner reads artist, release group, and label data from a MusicBrainz mirror, adds bios and images from other sources, and builds a similar-artists graph with an LLM. See [PLAN.md](PLAN.md) for the design and build order.

Status: the MusicBrainz endpoints and rate limiting work. Enrichment and the similar-artists graph are not built yet.

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
| `LASTFM_API_KEY`, `FANART_API_KEY` | unset |
| `LLM_BASE_URL` | `https://api.openai.com/v1` |
| `LLM_API_KEY` | unset |
| `LLM_MODEL` | `gpt-6.1-sol` |
| `LLM_DAILY_ARTIST_CAP` | `200` |

## Rate limits and API keys

Anonymous clients get `RATE_LIMIT_ANON_PER_MIN` requests per minute per IP. Clients that send a valid `X-Liner-Key` header get `RATE_LIMIT_KEY_PER_MIN`, or the key's own limit. Over the limit, liner returns `429` with `Retry-After`.

```bash
cargo run -- key create mu 2000   # prints the key once
```

## Deploy

CI builds the image and runs `pulumi up` in `infra/` on every push to `main`. The stack deploys onto the halfnotes `dev` cluster. It expects a `liner` role and database in the in-cluster Postgres, and the ingress, cert-manager, and `letsencrypt` issuer from the halfnotes stack.

## License

MIT
