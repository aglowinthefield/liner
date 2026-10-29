# liner: plan

liner is an open-source music metadata server at `liner.halfnotes.club`. It serves artist, release group, and label data from a MusicBrainz mirror, adds bios and images from other sources, and builds a similar-artists graph with an LLM. Anyone can self-host it. mu (the music player) and halfnotes are its first clients.

Lidarr's metadata server is a loose inspiration. liner doesn't copy its API.

## Decisions

- **Name:** liner, after liner notes.
- **Repo:** `aglowinthefield/liner`, separate from halfnotes. MIT license.
- **Stack:** the same as `halfnotes/api`: Rust, axum, sqlx with runtime queries, Redis, utoipa/Scalar, and migrations that run on startup.
- **Clients:** mu calls liner from a native app or a server, not from a browser, so liner has no CORS. halfnotes moves its MusicBrainz access into liner if halfnotes is revived.
- **LLM:** OpenAI through a service account key. The client speaks the OpenAI-compatible API, so self-hosters can point it at Ollama or another provider.
- **Access:** reads are public. Anonymous clients get a per-IP rate limit. mu and halfnotes send an API key that raises their limit.
- **Hosting:** the halfnotes DigitalOcean `dev` stack.

## Repo layout

```
liner/
  src/
    main.rs            # `liner serve` | `liner worker`
    lib.rs             # build_router, ApiDoc, AppState
    config.rs, db.rs, cache.rs        # cache.rs ported from halfnotes
    ratelimit.rs       # Redis-backed per-IP and per-key limits
    musicbrainz/       # client, ported and grown from halfnotes musicbrainz.rs
    artist/ release_group/ label/ search/   # ported from halfnotes */model.rs
    sources/           # lastfm.rs, listenbrainz.rs, wikipedia.rs, fanart.rs, caa.rs
    similar/           # candidates.rs, rerank.rs (LLM), handlers.rs
    llm.rs             # OpenAI-compatible client
    jobs.rs            # Postgres job queue
  migrations/
  infra/               # small Pulumi project (see "Infra")
  docker-compose.yml   # postgres + redis + liner, for self-hosters
```

## API (v1)

- `GET /v1/artist/{mbid}` returns MusicBrainz core data, the bio with its source and license, images, tags, and links.
- `GET /v1/artist/{mbid}/similar?limit=` returns ranked edges with `kind`, `reason`, and `sources`. It returns `status: pending` while the graph is being built.
- `GET /v1/graph/{mbid}?depth=2` returns nodes and edges for players to draw.
- `GET /v1/release-group/{mbid}`, `GET /v1/label/{mbid}`, and `GET /v1/search?q=` are ports of the halfnotes endpoints.
- `GET /v1/match?artist=&album=` replaces halfnotes' import matching.
- `POST /v1/entities/names` does batch name lookups. halfnotes uses it when it stores `entity_name` and `artist_name`.
- `POST /admin/refresh/{mbid}` forces a refetch. It requires `ADMIN_TOKEN`.

Entity types use MusicBrainz's hyphens (`release-group`). The halfnotes client translates them to its underscores.

## Rate limiting

- liner runs a fixed-window counter in Redis per client. The key is `liner:rl:{ip|key}:{minute}`.
- An anonymous client gets 60 requests per minute (`RATE_LIMIT_ANON_PER_MIN`). A client with a valid `X-Liner-Key` header gets 1200 per minute (`RATE_LIMIT_KEY_PER_MIN`).
- liner reads the client IP from `X-Forwarded-For`, which the nginx ingress sets. Self-hosters without a proxy set `TRUST_PROXY=false`.
- Over the limit, liner returns `429` with `Retry-After`.
- API keys live in an `api_keys` table and are stored hashed. An admin creates them with the `liner` CLI.

## Data model

- `source_docs (mbid, source, payload jsonb, fetched_at, expires_at)` holds the raw response from each source, so liner can reparse without refetching.
- `artist_bios (mbid, source, lang, summary, content, url, license, fetched_at)`
- `images (mbid, entity_type, kind, url, source, width, height)`
- `similar_candidates (from_mbid, to_mbid, source, raw_score, fetched_at)`
- `similar_edges (from_mbid, to_mbid, score, kind, reason, sources text[], model, prompt_version, generated_at)`
- `jobs (id, kind, mbid, status, attempts, run_after, last_error)`, polled with `FOR UPDATE SKIP LOCKED`, so liner needs no queue service.
- `api_keys (id, name, key_hash, rate_limit_per_min, created_at, revoked_at)`

Redis caches MusicBrainz and hot API responses. Postgres holds the enrichment data.

## How a request fills in

1. MusicBrainz core data comes synchronously from `mb.halfnotes.club` through the cache, as in halfnotes today.
2. On a cache miss, liner fetches the bio and images synchronously with a 3 s timeout per source. A source that times out becomes a job, and the response returns what liner already has.
3. Similar-artist work always runs as a job. A request for an unknown artist queues `collect_candidates`, then `rerank`.

## Similar-artists pipeline

1. Candidates come from three sources:
   - the ListenBrainz Labs similar-artists API
   - Last.fm `artist.getSimilar`
   - MusicBrainz relationships (shared members, collaborations, overlapping tags)

   liner normalises each source's score and merges the results into 30 to 50 candidates.
2. The rerank job sends OpenAI the artist's bio and tags, plus each candidate's name, tags, and the source that suggested it. It uses structured outputs with a JSON schema. The model returns a subset of candidates, each with a `score`, a `kind` (`members`, `collaborators`, `scene`, `sound`, `influence`, `label`), and a `reason` of 140 characters or less.
3. liner checks every returned MBID against the candidate set and drops the rest. The LLM can't add artists.
4. Configuration: `LLM_BASE_URL` (default `https://api.openai.com/v1`), `LLM_API_KEY`, and `LLM_MODEL` (`gpt-6.1-sol`; the model must support structured outputs).
5. Cost control: liner only reranks artists that a client requested, and `LLM_DAILY_ARTIST_CAP` limits how many per day (200 to start). When `prompt_version` changes, liner regenerates edges the next time someone requests them.
6. Without an LLM key, liner serves the merged candidate scores with no `reason`.

## Sources and licensing

- Wikipedia and Wikidata, found through MusicBrainz URL relationships. Text is CC-BY-SA, so liner stores attribution with every bio.
- Last.fm `artist.getInfo` and `artist.getSimilar`. Needs `LASTFM_API_KEY`. Bios are CC-BY-SA, and the API terms are non-commercial.
- ListenBrainz Labs similar artists. No key needed.
- fanart.tv for artist images. Needs `FANART_API_KEY`.
- Cover Art Archive for release-group covers.

## Infra

The halfnotes `dev` stack is running on cluster `halfnotes-dev-46c7aca` (see `docs/infra-shutdown-restore.md`).

- **Postgres:** a `liner` database in the existing in-cluster Postgres (`halfnotes-db`).
- **Redis:** the existing `halfnotes-redis`, with a `liner:` key prefix.
- **Deployments:** `liner-api` and `liner-worker` run the same image with different arguments. They get a Service, an Ingress for `liner.halfnotes.club` with the existing Let's Encrypt issuer, and a DNS record, as Grafana has.
- **Secrets:** Pulumi config secrets (`lastfmApiKey`, `fanartApiKey`, `openaiApiKey`, `adminToken`) become a K8s Secret.
- **Image:** `registry.digitalocean.com/halfnotes/liner:<sha>`, pushed by liner's CI.

The Pulumi project lives in `liner/infra`, not in `halfnotes/infra`. halfnotes deploys by running `pulumi up` in its own repo, filtered by path, so a liner image built in another repo can't trigger that cleanly. `liner/infra` uses the ambient kubeconfig (CI writes one with `doctl`), and liner's CI builds and deploys on its own. The `liner` Postgres role and database in `halfnotes-db` were created by hand with `kubectl exec`; they are not in Pulumi.

## Build order

1. **Scaffold.** Copy the halfnotes API skeleton (config, db, migrations, router, ApiDoc, metrics). Add `docker-compose.yml` and a CI job that builds and pushes the image.
2. **MusicBrainz layer.** Port `musicbrainz.rs`, `cache.rs`, and the artist, release, label, search, and match models with their integration tests.
3. **Deploy and rate limit.** Put `liner.halfnotes.club` live with step 2's endpoints and the rate limiter, so infra problems show up early.
4. **Enrichment.** Add Wikipedia and Wikidata, Last.fm bios and tags, fanart.tv, Cover Art Archive, and the job queue with its worker.
5. **Candidates.** `/similar` serves the merged candidates without ranking.
6. **LLM rerank.** Add edges, `/graph`, and the daily cap.
7. **mu integration.**
8. **halfnotes migration, if halfnotes is revived.** Replace `musicbrainz.rs` and `cache.rs` with a client configured by `LINER_URL`. The halfnotes models keep their DTOs, so the web app doesn't change.

## Open questions

- Whether `gpt-6.1-sol` supports structured outputs. Check before the rerank step (build order 6).
