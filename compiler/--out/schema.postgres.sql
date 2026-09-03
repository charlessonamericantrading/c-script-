-- Schema generado automáticamente por Link (PostgreSQL Enterprise Backend)

CREATE TABLE IF NOT EXISTS "authors" (
  "id" BIGSERIAL PRIMARY KEY,
  "name" TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS "posts" (
  "id" BIGSERIAL PRIMARY KEY,
  "title" TEXT NOT NULL,
  "authorId" BIGINT NOT NULL
);

DO $$ BEGIN IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'fk_posts_authorId') THEN ALTER TABLE "posts" ADD CONSTRAINT "fk_posts_authorId" FOREIGN KEY ("authorId") REFERENCES "authors"("id") ON DELETE CASCADE; END IF; END $$;