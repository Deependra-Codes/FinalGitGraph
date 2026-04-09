ALTER TABLE indexing_checkpoints
  ADD COLUMN identity_fingerprint TEXT;

ALTER TABLE indexing_checkpoints
  ADD COLUMN repository_topology TEXT;

ALTER TABLE indexing_checkpoints
  ADD COLUMN repository_is_bare INTEGER;

ALTER TABLE indexing_checkpoints
  ADD COLUMN repository_is_shallow INTEGER;

ALTER TABLE indexing_checkpoints
  ADD COLUMN repository_object_hash TEXT;

ALTER TABLE indexing_checkpoints
  ADD COLUMN repository_ref_storage TEXT;

ALTER TABLE indexing_checkpoints
  ADD COLUMN repository_has_sparse_checkout INTEGER;

ALTER TABLE indexing_checkpoints
  ADD COLUMN repository_has_partial_clone INTEGER;
