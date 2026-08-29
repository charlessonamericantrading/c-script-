// Generado automáticamente por linkc v1.136.0 — no editar a mano.

import { z } from "zod";

export const RoleSchema = z.enum(["Admin", "Member", "Guest"]);
export type Role = z.infer<typeof RoleSchema>;

export const PrioritySchema = z.enum(["High", "Medium", "Low"]);
export type Priority = z.infer<typeof PrioritySchema>;

export const ColumnIdSchema = z.enum(["Todo", "InProgress", "Done"]);
export type ColumnId = z.infer<typeof ColumnIdSchema>;

export const TaskSchema = z.object({
  id: z.number().int(),
  title: z.string(),
  description: z.string().nullable(),
  priority: PrioritySchema,
  column: ColumnIdSchema,
  assigneeEmail: z.string().nullable(),
  createdAt: z.string().datetime(),
});
export type Task = z.infer<typeof TaskSchema>;

export const NewTaskSchema = z.object({
  title: z.string(),
  description: z.string().nullable(),
  priority: PrioritySchema,
  column: ColumnIdSchema,
  assigneeEmail: z.string().nullable(),
});
export type NewTask = z.infer<typeof NewTaskSchema>;

export const BoardStatsSchema = z.object({
  total: z.number().int(),
  todoCount: z.number().int(),
  inProgressCount: z.number().int(),
  doneCount: z.number().int(),
});
export type BoardStats = z.infer<typeof BoardStatsSchema>;

