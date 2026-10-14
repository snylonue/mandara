-- Persistent opaque state owned by one registered plugin instance.
CREATE TABLE plugin_state (
    instance_id TEXT NOT NULL REFERENCES plugin_instances(id) ON DELETE CASCADE,
    key         TEXT NOT NULL,
    value       BLOB NOT NULL CHECK(length(value) <= 262144),
    PRIMARY KEY (instance_id, key)
);
