CREATE TABLE source_docs (
    mbid UUID NOT NULL,
    source TEXT NOT NULL,
    payload JSONB NOT NULL,
    fetched_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (mbid, source)
);

CREATE TABLE artist_bios (
    mbid UUID NOT NULL,
    source TEXT NOT NULL,
    lang TEXT NOT NULL DEFAULT 'en',
    summary TEXT NOT NULL,
    content TEXT,
    url TEXT,
    license TEXT,
    fetched_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (mbid, source, lang)
);

CREATE TABLE images (
    id BIGSERIAL PRIMARY KEY,
    mbid UUID NOT NULL,
    entity_type TEXT NOT NULL,
    kind TEXT NOT NULL,
    url TEXT NOT NULL,
    source TEXT NOT NULL,
    width INT,
    height INT,
    UNIQUE (mbid, kind, url)
);

CREATE TABLE similar_candidates (
    from_mbid UUID NOT NULL,
    to_mbid UUID NOT NULL,
    source TEXT NOT NULL,
    raw_score DOUBLE PRECISION NOT NULL,
    fetched_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (from_mbid, to_mbid, source)
);

CREATE TABLE similar_edges (
    from_mbid UUID NOT NULL,
    to_mbid UUID NOT NULL,
    score DOUBLE PRECISION NOT NULL,
    kind TEXT NOT NULL,
    reason TEXT,
    sources TEXT[] NOT NULL DEFAULT '{}',
    model TEXT,
    prompt_version INT,
    generated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (from_mbid, to_mbid)
);

CREATE TABLE jobs (
    id BIGSERIAL PRIMARY KEY,
    kind TEXT NOT NULL,
    mbid UUID NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    attempts INT NOT NULL DEFAULT 0,
    run_after TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX jobs_pending_idx ON jobs (run_after) WHERE status = 'pending';
CREATE UNIQUE INDEX jobs_dedupe_idx ON jobs (kind, mbid) WHERE status IN ('pending', 'running');

CREATE TABLE api_keys (
    id BIGSERIAL PRIMARY KEY,
    name TEXT NOT NULL,
    key_hash TEXT NOT NULL UNIQUE,
    rate_limit_per_min INT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at TIMESTAMPTZ
);
