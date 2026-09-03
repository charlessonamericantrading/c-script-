// Generado automáticamente por linkc v1.200.1 — no editar a mano.

import type { Author, Post } from "./contract";

export function isPost(x: unknown): x is Post {
  return (typeof x === "object" && x !== null && !Array.isArray(x) && (typeof (x as any).id === "number" && Number.isInteger((x as any).id)) && typeof (x as any).title === "string" && (typeof (x as any).authorId === "number" && Number.isInteger((x as any).authorId)));
}

export function isAuthor(x: unknown): x is Author {
  return (typeof x === "object" && x !== null && !Array.isArray(x) && (typeof (x as any).id === "number" && Number.isInteger((x as any).id)) && typeof (x as any).name === "string");
}

