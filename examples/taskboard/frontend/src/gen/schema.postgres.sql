-- Schema generado automáticamente por Link (PostgreSQL Enterprise Backend)

CREATE TABLE IF NOT EXISTS "tasks" (
  "id" BIGSERIAL PRIMARY KEY,
  "title" TEXT NOT NULL,
  "description" TEXT,
  "priority" TEXT NOT NULL,
  "column" TEXT NOT NULL,
  "assigneeEmail" TEXT,
  "createdAt" BIGINT NOT NULL
);