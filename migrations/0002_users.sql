CREATE TABLE incisive.users (
    id BIGSERIAL PRIMARY KEY,
    email TEXT NOT NULL UNIQUE CHECK (email = lower(email)),
    display_name TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('owner','surgeon','fellow')),
    daily_cap_paise BIGINT NOT NULL,
    monthly_cap_paise BIGINT,
    active BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
