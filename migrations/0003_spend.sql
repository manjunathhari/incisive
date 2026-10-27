-- Spending ledger. Money is integer paise; never floats.
-- `reservation` rows hold an estimate while a request is in flight; `settle`
-- replaces them with the actual `charge`.
CREATE TABLE incisive.spend (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES incisive.users(id),
    at TIMESTAMPTZ NOT NULL DEFAULT now(),
    kind TEXT NOT NULL CHECK (kind IN ('reservation','charge')),
    purpose TEXT NOT NULL,
    model TEXT,
    input_tokens INT,
    output_tokens INT,
    paise BIGINT NOT NULL,
    request_id UUID NOT NULL
);

CREATE INDEX spend_user_at_idx ON incisive.spend (user_id, at);
