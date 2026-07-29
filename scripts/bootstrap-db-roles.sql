-- Provision the two database roles the runtime requires.
--
-- `assert_safe_runtime_role` (crates/tenant-context/src/lib.rs) refuses to start
-- any runtime binary whose role can bypass row-level security or owns a tenant
-- table, because either condition silently defeats the forced-RLS isolation that
-- migrations 0016 and 0017 establish. That demands two distinct roles:
--
--   perpeto_migrator  owns the schema and runs funding-arb-migrate
--   perpeto_app       owns nothing, holds only DML, and runs every service
--
-- Role creation is a cluster-level operation, so it lives here rather than in a
-- sqlx migration. Run this as a superuser against the application database
-- before the first migration. Postgres executes files in
-- /docker-entrypoint-initdb.d automatically on first initialization, which is
-- how the dev Compose stack applies it.
--
-- Passwords are development defaults. Override them in any deployed environment.

\set ON_ERROR_STOP on

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'perpeto_migrator') THEN
        CREATE ROLE perpeto_migrator LOGIN PASSWORD 'perpeto_migrator'
            NOSUPERUSER NOBYPASSRLS NOCREATEROLE NOCREATEDB;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'perpeto_app') THEN
        CREATE ROLE perpeto_app LOGIN PASSWORD 'perpeto_app'
            NOSUPERUSER NOBYPASSRLS NOCREATEROLE NOCREATEDB;
    END IF;
END
$$;

-- The migrator owns the schema so it can create tables; PostgreSQL 15+ no longer
-- grants CREATE on `public` to PUBLIC, so ownership is the clean way to allow it.
ALTER SCHEMA public OWNER TO perpeto_migrator;

-- CONNECT must name the database literally, so resolve it at runtime rather than
-- hardcoding it or depending on a psql variable the init hook does not set.
DO $$
BEGIN
    EXECUTE format(
        'GRANT CONNECT ON DATABASE %I TO perpeto_migrator, perpeto_app',
        current_database()
    );
END
$$;

GRANT USAGE ON SCHEMA public TO perpeto_app;

-- Existing objects, for a database that already has tables when this runs.
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO perpeto_app;
GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO perpeto_app;
GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA public TO perpeto_app;

-- Objects future migrations create. Without this, every new table silently locks
-- the runtime role out until someone notices in production.
ALTER DEFAULT PRIVILEGES FOR ROLE perpeto_migrator IN SCHEMA public
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO perpeto_app;
ALTER DEFAULT PRIVILEGES FOR ROLE perpeto_migrator IN SCHEMA public
    GRANT USAGE, SELECT ON SEQUENCES TO perpeto_app;
ALTER DEFAULT PRIVILEGES FOR ROLE perpeto_migrator IN SCHEMA public
    GRANT EXECUTE ON FUNCTIONS TO perpeto_app;
