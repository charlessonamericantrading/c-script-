// Generado automáticamente por linkc — no editar a mano.

import type { BoardStats, ColumnId, NewTask, Patch, Task, TasksClient } from "./contract";
import { isBoardStats, isTask } from "./validators.ts";

export class LinkTransportError extends Error {}

export class LinkValidationError extends Error {
  rpcName: string;
  received: unknown;
  constructor(rpcName: string, received: unknown) {
    super(`la respuesta de '${rpcName}' no matchea el contrato declarado`);
    this.rpcName = rpcName;
    this.received = received;
  }
}

export function isOk<T, E>(result: { ok: true; value: T } | { ok: false; error: E }): result is { ok: true; value: T } {
  return result.ok === true;
}

export function isErr<T, E>(result: { ok: true; value: T } | { ok: false; error: E }): result is { ok: false; error: E } {
  return result.ok === false;
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

  async list(): Promise<Task[]> {
    const res = await fetch(`${this.baseUrl}/Tasks/list`, {
      method: "POST",
      headers: { "Content-Type": "application/json", ...(this.token ? { Authorization: `Bearer ${this.token}` } : {}) },
      body: JSON.stringify({  }),
    });
    if (!res.ok) throw new LinkTransportError(`HTTP ${res.status}`);
    const json: unknown = await res.json();
    if (!((Array.isArray(json) && json.every((item: unknown) => isTask(item))))) throw new LinkValidationError("list", json);
    return json as Task[];
  }

  async getById(id: number): Promise<Task | null> {
    const res = await fetch(`${this.baseUrl}/Tasks/getById`, {
      method: "POST",
      headers: { "Content-Type": "application/json", ...(this.token ? { Authorization: `Bearer ${this.token}` } : {}) },
      body: JSON.stringify({ id }),
    });
    if (!res.ok) throw new LinkTransportError(`HTTP ${res.status}`);
    const json: unknown = await res.json();
    if (!((json === null || isTask(json)))) throw new LinkValidationError("getById", json);
    return json as Task | null;
  }

  async create(input: NewTask): Promise<Task> {
    const res = await fetch(`${this.baseUrl}/Tasks/create`, {
      method: "POST",
      headers: { "Content-Type": "application/json", ...(this.token ? { Authorization: `Bearer ${this.token}` } : {}) },
      body: JSON.stringify({ input }),
    });
    if (!res.ok) throw new LinkTransportError(`HTTP ${res.status}`);
    const json: unknown = await res.json();
    if (!(isTask(json))) throw new LinkValidationError("create", json);
    return json as Task;
  }

  async update(id: number, patch: Patch<Task>): Promise<Task> {
    const res = await fetch(`${this.baseUrl}/Tasks/update`, {
      method: "POST",
      headers: { "Content-Type": "application/json", ...(this.token ? { Authorization: `Bearer ${this.token}` } : {}) },
      body: JSON.stringify({ id, patch }),
    });
    if (!res.ok) throw new LinkTransportError(`HTTP ${res.status}`);
    const json: unknown = await res.json();
    if (!(isTask(json))) throw new LinkValidationError("update", json);
    return json as Task;
  }

  async remove(id: number): Promise<boolean> {
    const res = await fetch(`${this.baseUrl}/Tasks/remove`, {
      method: "POST",
      headers: { "Content-Type": "application/json", ...(this.token ? { Authorization: `Bearer ${this.token}` } : {}) },
      body: JSON.stringify({ id }),
    });
    if (!res.ok) throw new LinkTransportError(`HTTP ${res.status}`);
    const json: unknown = await res.json();
    if (!(typeof json === "boolean")) throw new LinkValidationError("remove", json);
    return json as boolean;
  }

  async listByColumn(col: ColumnId): Promise<Task[]> {
    const res = await fetch(`${this.baseUrl}/Tasks/listByColumn`, {
      method: "POST",
      headers: { "Content-Type": "application/json", ...(this.token ? { Authorization: `Bearer ${this.token}` } : {}) },
      body: JSON.stringify({ col }),
    });
    if (!res.ok) throw new LinkTransportError(`HTTP ${res.status}`);
    const json: unknown = await res.json();
    if (!((Array.isArray(json) && json.every((item: unknown) => isTask(item))))) throw new LinkValidationError("listByColumn", json);
    return json as Task[];
  }

  async stats(): Promise<BoardStats> {
    const res = await fetch(`${this.baseUrl}/Tasks/stats`, {
      method: "POST",
      headers: { "Content-Type": "application/json", ...(this.token ? { Authorization: `Bearer ${this.token}` } : {}) },
      body: JSON.stringify({  }),
    });
    if (!res.ok) throw new LinkTransportError(`HTTP ${res.status}`);
    const json: unknown = await res.json();
    if (!(isBoardStats(json))) throw new LinkValidationError("stats", json);
    return json as BoardStats;
  }

  async *watchTasks(): AsyncIterable<Task> {
    const res = await fetch(`${this.baseUrl}/Tasks/watchTasks`, {
      method: "POST",
      headers: { "Content-Type": "application/json", ...(this.token ? { Authorization: `Bearer ${this.token}` } : {}) },
      body: JSON.stringify({  }),
    });
    if (!res.ok) throw new LinkTransportError(`HTTP ${res.status}`);
    if (!res.body) throw new LinkTransportError("el servidor no devolvió un body de stream");
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
