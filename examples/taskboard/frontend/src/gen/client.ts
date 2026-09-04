// Generado automáticamente por linkc v1.202.0 — no editar a mano.

import type { BoardStats, ColumnId, NewTask, Patch, Result, Task, TasksClient } from "./contract";
import { isBoardStats, isTask } from "./validators.ts";

export class LinkTransportError extends Error {
  status: number;
  constructor(message: string, status: number) {
    super(message);
    this.status = status;
  }
}

export class LinkValidationError extends Error {
  rpcName: string;
  received: unknown;
  constructor(rpcName: string, received: unknown) {
    super(`la respuesta de '${rpcName}' no matchea el contrato declarado`);
    this.rpcName = rpcName;
    this.received = received;
  }
}

export function isOk<T, E>(result: Result<T, E>): result is { type: "Ok"; value: T } {
  return result.type === "Ok";
}

export function isErr<T, E>(result: Result<T, E>): result is { type: "Err"; error: E } {
  return result.type === "Err";
}

class TasksClientImpl implements TasksClient {
  private baseUrl: string;
  private token: string | null = null;
  constructor(baseUrl: string) {
    this.baseUrl = baseUrl;
  }

  setToken(token: string | null): void {
    this.token = token;
  }

  async list(options?: { signal?: AbortSignal }): Promise<Task[]> {
    const res = await fetch(`${this.baseUrl}/Tasks/list`, {
      method: "POST",
      headers: { "Content-Type": "application/json", ...(this.token ? { Authorization: `Bearer ${this.token}` } : {}) },
      body: JSON.stringify({  }),
      signal: options?.signal,
    });
    if (!res.ok) throw new LinkTransportError(`HTTP ${res.status}`, res.status);
    const json: unknown = await res.json();
    if (!((Array.isArray(json) && json.every((item: unknown) => isTask(item))))) throw new LinkValidationError("list", json);
    return json as Task[];
  }

  async listPaged(cursor: number | null, limit: number, options?: { signal?: AbortSignal }): Promise<Task[]> {
    const res = await fetch(`${this.baseUrl}/Tasks/listPaged`, {
      method: "POST",
      headers: { "Content-Type": "application/json", ...(this.token ? { Authorization: `Bearer ${this.token}` } : {}) },
      body: JSON.stringify({ cursor, limit }),
      signal: options?.signal,
    });
    if (!res.ok) throw new LinkTransportError(`HTTP ${res.status}`, res.status);
    const json: unknown = await res.json();
    if (!((Array.isArray(json) && json.every((item: unknown) => isTask(item))))) throw new LinkValidationError("listPaged", json);
    return json as Task[];
  }

  async getById(id: number, options?: { signal?: AbortSignal }): Promise<Task | null> {
    const res = await fetch(`${this.baseUrl}/Tasks/getById`, {
      method: "POST",
      headers: { "Content-Type": "application/json", ...(this.token ? { Authorization: `Bearer ${this.token}` } : {}) },
      body: JSON.stringify({ id }),
      signal: options?.signal,
    });
    if (!res.ok) throw new LinkTransportError(`HTTP ${res.status}`, res.status);
    const json: unknown = await res.json();
    if (!((json === null || isTask(json)))) throw new LinkValidationError("getById", json);
    return json as Task | null;
  }

  async create(input: NewTask, options?: { signal?: AbortSignal }): Promise<Task> {
    const res = await fetch(`${this.baseUrl}/Tasks/create`, {
      method: "POST",
      headers: { "Content-Type": "application/json", ...(this.token ? { Authorization: `Bearer ${this.token}` } : {}) },
      body: JSON.stringify({ input }),
      signal: options?.signal,
    });
    if (!res.ok) throw new LinkTransportError(`HTTP ${res.status}`, res.status);
    const json: unknown = await res.json();
    if (!(isTask(json))) throw new LinkValidationError("create", json);
    return json as Task;
  }

  async update(id: number, patch: Patch<Task>, options?: { signal?: AbortSignal }): Promise<Task> {
    const res = await fetch(`${this.baseUrl}/Tasks/update`, {
      method: "POST",
      headers: { "Content-Type": "application/json", ...(this.token ? { Authorization: `Bearer ${this.token}` } : {}) },
      body: JSON.stringify({ id, patch }),
      signal: options?.signal,
    });
    if (!res.ok) throw new LinkTransportError(`HTTP ${res.status}`, res.status);
    const json: unknown = await res.json();
    if (!(isTask(json))) throw new LinkValidationError("update", json);
    return json as Task;
  }

  async remove(id: number, options?: { signal?: AbortSignal }): Promise<boolean> {
    const res = await fetch(`${this.baseUrl}/Tasks/remove`, {
      method: "POST",
      headers: { "Content-Type": "application/json", ...(this.token ? { Authorization: `Bearer ${this.token}` } : {}) },
      body: JSON.stringify({ id }),
      signal: options?.signal,
    });
    if (!res.ok) throw new LinkTransportError(`HTTP ${res.status}`, res.status);
    const json: unknown = await res.json();
    if (!(typeof json === "boolean")) throw new LinkValidationError("remove", json);
    return json as boolean;
  }

  async listByColumn(col: ColumnId, options?: { signal?: AbortSignal }): Promise<Task[]> {
    const res = await fetch(`${this.baseUrl}/Tasks/listByColumn`, {
      method: "POST",
      headers: { "Content-Type": "application/json", ...(this.token ? { Authorization: `Bearer ${this.token}` } : {}) },
      body: JSON.stringify({ col }),
      signal: options?.signal,
    });
    if (!res.ok) throw new LinkTransportError(`HTTP ${res.status}`, res.status);
    const json: unknown = await res.json();
    if (!((Array.isArray(json) && json.every((item: unknown) => isTask(item))))) throw new LinkValidationError("listByColumn", json);
    return json as Task[];
  }

  async stats(options?: { signal?: AbortSignal }): Promise<BoardStats> {
    const res = await fetch(`${this.baseUrl}/Tasks/stats`, {
      method: "POST",
      headers: { "Content-Type": "application/json", ...(this.token ? { Authorization: `Bearer ${this.token}` } : {}) },
      body: JSON.stringify({  }),
      signal: options?.signal,
    });
    if (!res.ok) throw new LinkTransportError(`HTTP ${res.status}`, res.status);
    const json: unknown = await res.json();
    if (!(isBoardStats(json))) throw new LinkValidationError("stats", json);
    return json as BoardStats;
  }

  async *watchTasks(options?: { signal?: AbortSignal }): AsyncIterable<Task> {
    const res = await fetch(`${this.baseUrl}/Tasks/watchTasks`, {
      method: "POST",
      headers: { "Content-Type": "application/json", ...(this.token ? { Authorization: `Bearer ${this.token}` } : {}) },
      body: JSON.stringify({  }),
      signal: options?.signal,
    });
    if (!res.ok) throw new LinkTransportError(`HTTP ${res.status}`, res.status);
    if (!res.body) throw new LinkTransportError("el servidor no devolvió un body de stream", res.status);
    const reader = res.body.getReader();
    const decoder = new TextDecoder();
    let buffer = "";
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      buffer += decoder.decode(value, { stream: true });
      let sep: number;
      while ((sep = buffer.indexOf("\n\n")) !== -1) {
        const frame = buffer.slice(0, sep);
        buffer = buffer.slice(sep + 2);
        if (!frame.startsWith("data: ")) continue;
        const json: unknown = JSON.parse(frame.slice(6));
        if (!(isTask(json))) throw new LinkValidationError("watchTasks", json);
        yield json as Task;
      }
    }
  }

}

export function createTasksClient(baseUrl: string): TasksClient {
  return new TasksClientImpl(baseUrl);
}

