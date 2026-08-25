// Generado automáticamente por linkc v1.96.0 — no editar a mano.

export type Result<T, E> = { type: "Ok"; value: T } | { type: "Err"; error: E };
export type Patch<T> = Partial<T>;

export type Role = "Admin" | "Member" | "Guest";

export type Priority = "High" | "Medium" | "Low";

export type ColumnId = "Todo" | "InProgress" | "Done";

export interface Task {
  id: number;
  title: string;
  description: string | null;
  priority: Priority;
  column: ColumnId;
  assigneeEmail: string | null;
  createdAt: string;
}

export interface NewTask {
  title: string;
  description: string | null;
  priority: Priority;
  column: ColumnId;
  assigneeEmail: string | null;
}

export interface BoardStats {
  total: number;
  todoCount: number;
  inProgressCount: number;
  doneCount: number;
}

export interface TasksClient {
  list(options?: { signal?: AbortSignal }): Promise<Task[]>;
  getById(id: number, options?: { signal?: AbortSignal }): Promise<Task | null>;
  create(input: NewTask, options?: { signal?: AbortSignal }): Promise<Task>;
  update(id: number, patch: Patch<Task>, options?: { signal?: AbortSignal }): Promise<Task>;
  remove(id: number, options?: { signal?: AbortSignal }): Promise<boolean>;
  listByColumn(col: ColumnId, options?: { signal?: AbortSignal }): Promise<Task[]>;
  stats(options?: { signal?: AbortSignal }): Promise<BoardStats>;
  watchTasks(options?: { signal?: AbortSignal }): AsyncIterable<Task>;
  setToken(token: string | null): void;
}

