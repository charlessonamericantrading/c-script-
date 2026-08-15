// Generado automáticamente por linkc — no editar a mano.

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
  list(): Promise<Task[]>;
  getById(id: number): Promise<Task | null>;
  create(input: NewTask): Promise<Task>;
  update(id: number, patch: Patch<Task>): Promise<Task>;
  remove(id: number): Promise<boolean>;
  listByColumn(col: ColumnId): Promise<Task[]>;
  stats(): Promise<BoardStats>;
  watchTasks(): AsyncIterable<Task>;
  setToken(token: string | null): void;
}

