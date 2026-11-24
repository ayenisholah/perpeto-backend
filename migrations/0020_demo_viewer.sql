-- DEC-0017: the public demo reaches one seeded demo tenant through a single
-- read-only principal. The flag marks that principal; the API refuses every
-- non-GET request it makes, whatever roles it holds.
ALTER TABLE users ADD COLUMN demo boolean NOT NULL DEFAULT false;

-- At most one demo principal per deployment.
CREATE UNIQUE INDEX one_demo_user ON users ((demo)) WHERE demo;

-- The demo is served to browsers, so its device is a web client.
ALTER TABLE devices DROP CONSTRAINT devices_platform_check;
ALTER TABLE devices ADD CONSTRAINT devices_platform_check CHECK (platform IN ('IOS', 'WEB'));
