// Generado automáticamente por linkc v1.200.1 — no editar a mano.

export type Result<T, E> = { type: "Ok"; value: T } | { type: "Err"; error: E };
export type Patch<T> = Partial<T>;

export type PdfBlock =
  | { type: "Text"; content: string; bold: boolean; size: number }
  | { type: "Table"; headers: string[]; rows: string[][] }
;

export type ExcelCell =
  | { type: "Text"; value: string }
  | { type: "Number"; value: string }
  | { type: "Date"; value: string }
  | { type: "Bool"; value: boolean }
  | { type: "Empty" }
;

export interface ExcelSheet {
  name: string;
  headers: string[];
  rows: ExcelCell[][];
}

export interface AiMessage {
  role: string;
  content: string;
}

export interface AiToken {
  token: string;
  done: boolean;
}

export interface Author {
  id: number;
  name: string;
}

export interface Post {
  id: number;
  title: string;
  authorId: number;
}

export interface BlogClient {
  createAuthor(name: string, options?: { signal?: AbortSignal }): Promise<Author>;
  createPost(title: string, authorId: number, options?: { signal?: AbortSignal }): Promise<Post>;
  deleteAuthor(id: number, options?: { signal?: AbortSignal }): Promise<boolean>;
  setToken(token: string | null): void;
}

