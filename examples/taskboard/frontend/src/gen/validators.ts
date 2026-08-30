// Generado automáticamente por linkc v1.150.0 — no editar a mano.

import type { BoardStats, ColumnId, NewTask, Patch, Priority, Task } from "./contract";

export function isBoardStats(x: unknown): x is BoardStats {
  return (typeof x === "object" && x !== null && !Array.isArray(x) && (typeof (x as any).total === "number" && Number.isInteger((x as any).total)) && (typeof (x as any).todoCount === "number" && Number.isInteger((x as any).todoCount)) && (typeof (x as any).inProgressCount === "number" && Number.isInteger((x as any).inProgressCount)) && (typeof (x as any).doneCount === "number" && Number.isInteger((x as any).doneCount)));
}

export function isColumnId(x: unknown): x is ColumnId {
  return (x === "Todo" || x === "InProgress" || x === "Done");
}

export function isPatch_Task(x: unknown): x is Patch<Task> {
  return (typeof x === "object" && x !== null && !Array.isArray(x) && ((x as any).id === undefined || (typeof (x as any).id === "number" && Number.isInteger((x as any).id))) && ((x as any).title === undefined || typeof (x as any).title === "string") && ((x as any).description === undefined || ((x as any).description === null || typeof (x as any).description === "string")) && ((x as any).priority === undefined || isPriority((x as any).priority)) && ((x as any).column === undefined || isColumnId((x as any).column)) && ((x as any).assigneeEmail === undefined || ((x as any).assigneeEmail === null || typeof (x as any).assigneeEmail === "string")) && ((x as any).createdAt === undefined || (typeof (x as any).createdAt === "string" && /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/.test((x as any).createdAt) && !isNaN(Date.parse((x as any).createdAt)))));
}

export function isPriority(x: unknown): x is Priority {
  return (x === "High" || x === "Medium" || x === "Low");
}

export function isNewTask(x: unknown): x is NewTask {
  return (typeof x === "object" && x !== null && !Array.isArray(x) && typeof (x as any).title === "string" && ((x as any).description === null || typeof (x as any).description === "string") && isPriority((x as any).priority) && isColumnId((x as any).column) && ((x as any).assigneeEmail === null || typeof (x as any).assigneeEmail === "string"));
}

export function isTask(x: unknown): x is Task {
  return (typeof x === "object" && x !== null && !Array.isArray(x) && (typeof (x as any).id === "number" && Number.isInteger((x as any).id)) && typeof (x as any).title === "string" && ((x as any).description === null || typeof (x as any).description === "string") && isPriority((x as any).priority) && isColumnId((x as any).column) && ((x as any).assigneeEmail === null || typeof (x as any).assigneeEmail === "string") && (typeof (x as any).createdAt === "string" && /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/.test((x as any).createdAt) && !isNaN(Date.parse((x as any).createdAt))));
}

