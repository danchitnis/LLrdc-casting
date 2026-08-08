CREATE TABLE IF NOT EXISTS active_receivers (
    receiver_id TEXT PRIMARY KEY NOT NULL,
    pairing_code TEXT UNIQUE,
    ip_address TEXT NOT NULL,
    webtransport_port INTEGER NOT NULL,
    cert_hash_hex TEXT NOT NULL,
    code_expires_at INTEGER NOT NULL,
    registration_expires_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS active_receivers_code_expiry_idx
    ON active_receivers (code_expires_at);

CREATE INDEX IF NOT EXISTS active_receivers_registration_expiry_idx
    ON active_receivers (registration_expires_at);

CREATE TABLE IF NOT EXISTS registration_replays (
    receiver_id TEXT NOT NULL,
    nonce TEXT NOT NULL,
    expires_at INTEGER NOT NULL,
    PRIMARY KEY (receiver_id, nonce)
);

CREATE INDEX IF NOT EXISTS registration_replays_expiry_idx
    ON registration_replays (expires_at);

CREATE TABLE IF NOT EXISTS rate_limits (
    bucket_key TEXT PRIMARY KEY NOT NULL,
    window_started_at INTEGER NOT NULL,
    attempt_count INTEGER NOT NULL
);
