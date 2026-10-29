ALTER TABLE jobs ADD COLUMN updated_at TIMESTAMPTZ NOT NULL DEFAULT now();
CREATE INDEX jobs_finished_idx ON jobs (updated_at) WHERE status IN ('done', 'failed');
