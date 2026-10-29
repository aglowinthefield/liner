ALTER TABLE similar_candidates ADD COLUMN to_name TEXT NOT NULL DEFAULT '';
ALTER TABLE similar_edges ADD COLUMN to_name TEXT NOT NULL DEFAULT '';

CREATE TABLE artist_tags (
    mbid UUID NOT NULL,
    source TEXT NOT NULL,
    name TEXT NOT NULL,
    position INT NOT NULL,
    PRIMARY KEY (mbid, source, name)
);
