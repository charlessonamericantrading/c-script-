// Generado automáticamente por linkc v1.200.1 — no editar a mano.

import type { Author, BlogClient, Post, Result } from "./contract";
import { isAuthor, isPost } from "./validators.ts";

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

class BlogClientImpl implements BlogClient {
  private baseUrl: string;
  private token: string | null = null;
  constructor(baseUrl: string) {
    this.baseUrl = baseUrl;
  }

  setToken(token: string | null): void {
    this.token = token;
  }

  async createAuthor(name: string, options?: { signal?: AbortSignal }): Promise<Author> {
    const res = await fetch(`${this.baseUrl}/Blog/createAuthor`, {
      method: "POST",
      headers: { "Content-Type": "application/json", ...(this.token ? { Authorization: `Bearer ${this.token}` } : {}) },
      body: JSON.stringify({ name }),
      signal: options?.signal,
    });
    if (!res.ok) throw new LinkTransportError(`HTTP ${res.status}`, res.status);
    const json: unknown = await res.json();
    if (!(isAuthor(json))) throw new LinkValidationError("createAuthor", json);
    return json as Author;
  }

  async createPost(title: string, authorId: number, options?: { signal?: AbortSignal }): Promise<Post> {
    const res = await fetch(`${this.baseUrl}/Blog/createPost`, {
      method: "POST",
      headers: { "Content-Type": "application/json", ...(this.token ? { Authorization: `Bearer ${this.token}` } : {}) },
      body: JSON.stringify({ title, authorId }),
      signal: options?.signal,
    });
    if (!res.ok) throw new LinkTransportError(`HTTP ${res.status}`, res.status);
    const json: unknown = await res.json();
    if (!(isPost(json))) throw new LinkValidationError("createPost", json);
    return json as Post;
  }

  async deleteAuthor(id: number, options?: { signal?: AbortSignal }): Promise<boolean> {
    const res = await fetch(`${this.baseUrl}/Blog/deleteAuthor`, {
      method: "POST",
      headers: { "Content-Type": "application/json", ...(this.token ? { Authorization: `Bearer ${this.token}` } : {}) },
      body: JSON.stringify({ id }),
      signal: options?.signal,
    });
    if (!res.ok) throw new LinkTransportError(`HTTP ${res.status}`, res.status);
    const json: unknown = await res.json();
    if (!(typeof json === "boolean")) throw new LinkValidationError("deleteAuthor", json);
    return json as boolean;
  }

}

export function createBlogClient(baseUrl: string): BlogClient {
  return new BlogClientImpl(baseUrl);
}

