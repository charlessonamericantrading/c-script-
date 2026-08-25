// Generado automáticamente por linkc v1.86.0 — no editar a mano.

import { useState, useEffect, useCallback, useRef } from "react";
import type { BoardStats, ColumnId, NewTask, Patch, Task, TasksClient } from "./contract";

export interface QueryState<T> {
  data: T | null;
  loading: boolean;
  error: Error | null;
  refetch: () => Promise<T | null>;
}

export interface MutationState<T> {
  data: T | null;
  loading: boolean;
  error: Error | null;
  reset: () => void;
}

export interface SubscriptionState<T> {
  data: T[];
  latest: T | null;
  isConnected: boolean;
  error: Error | null;
}

export function useTasksListQuery(client: TasksClient, options?: { enabled?: boolean }): QueryState<Task[]> {
  const enabled = options?.enabled ?? true;
  const [data, setData] = useState<Task[] | null>(null);
  const [loading, setLoading] = useState(enabled);
  const [error, setError] = useState<Error | null>(null);
  const requestIdRef = useRef(0);

  const refetch = useCallback(async (): Promise<Task[] | null> => {
    const requestId = ++requestIdRef.current;
    setLoading(true);
    setError(null);
    try {
      const res = await client.list();
      if (requestIdRef.current === requestId) setData(res);
      return res;
    } catch (err) {
      const e = err instanceof Error ? err : new Error(String(err));
      if (requestIdRef.current === requestId) setError(e);
      return null;
    } finally {
      if (requestIdRef.current === requestId) setLoading(false);
    }
  }, [client]);

  useEffect(() => {
    if (enabled) {
      refetch();
    }
    return () => { requestIdRef.current++; };
  }, [enabled, refetch]);

  return { data, loading, error, refetch };
}

export function useTasksListMutation(client: TasksClient): MutationState<Task[]> & {
  mutate: () => Promise<Task[]>;
} {
  const [data, setData] = useState<Task[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<Error | null>(null);
  const requestIdRef = useRef(0);

  const mutate = useCallback(async (): Promise<Task[]> => {
    const requestId = ++requestIdRef.current;
    setLoading(true);
    setError(null);
    try {
      const res = await client.list();
      if (requestIdRef.current === requestId) setData(res);
      return res;
    } catch (err) {
      const e = err instanceof Error ? err : new Error(String(err));
      if (requestIdRef.current === requestId) setError(e);
      throw e;
    } finally {
      if (requestIdRef.current === requestId) setLoading(false);
    }
  }, [client]);

  const reset = useCallback(() => {
    requestIdRef.current++;
    setData(null);
    setLoading(false);
    setError(null);
  }, []);

  return { mutate, data, loading, error, reset };
}

export function useTasksGetByIdQuery(client: TasksClient, id: number, options?: { enabled?: boolean }): QueryState<Task | null> {
  const enabled = options?.enabled ?? true;
  const [data, setData] = useState<Task | null | null>(null);
  const [loading, setLoading] = useState(enabled);
  const [error, setError] = useState<Error | null>(null);
  const requestIdRef = useRef(0);

  const refetch = useCallback(async (): Promise<Task | null | null> => {
    const requestId = ++requestIdRef.current;
    setLoading(true);
    setError(null);
    try {
      const res = await client.getById(id);
      if (requestIdRef.current === requestId) setData(res);
      return res;
    } catch (err) {
      const e = err instanceof Error ? err : new Error(String(err));
      if (requestIdRef.current === requestId) setError(e);
      return null;
    } finally {
      if (requestIdRef.current === requestId) setLoading(false);
    }
  }, [client, id]);

  useEffect(() => {
    if (enabled) {
      refetch();
    }
    return () => { requestIdRef.current++; };
  }, [enabled, refetch]);

  return { data, loading, error, refetch };
}

export function useTasksGetByIdMutation(client: TasksClient): MutationState<Task | null> & {
  mutate: (id: number) => Promise<Task | null>;
} {
  const [data, setData] = useState<Task | null | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<Error | null>(null);
  const requestIdRef = useRef(0);

  const mutate = useCallback(async (id: number): Promise<Task | null> => {
    const requestId = ++requestIdRef.current;
    setLoading(true);
    setError(null);
    try {
      const res = await client.getById(id);
      if (requestIdRef.current === requestId) setData(res);
      return res;
    } catch (err) {
      const e = err instanceof Error ? err : new Error(String(err));
      if (requestIdRef.current === requestId) setError(e);
      throw e;
    } finally {
      if (requestIdRef.current === requestId) setLoading(false);
    }
  }, [client]);

  const reset = useCallback(() => {
    requestIdRef.current++;
    setData(null);
    setLoading(false);
    setError(null);
  }, []);

  return { mutate, data, loading, error, reset };
}

export function useTasksCreateMutation(client: TasksClient): MutationState<Task> & {
  mutate: (input: NewTask) => Promise<Task>;
} {
  const [data, setData] = useState<Task | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<Error | null>(null);
  const requestIdRef = useRef(0);

  const mutate = useCallback(async (input: NewTask): Promise<Task> => {
    const requestId = ++requestIdRef.current;
    setLoading(true);
    setError(null);
    try {
      const res = await client.create(input);
      if (requestIdRef.current === requestId) setData(res);
      return res;
    } catch (err) {
      const e = err instanceof Error ? err : new Error(String(err));
      if (requestIdRef.current === requestId) setError(e);
      throw e;
    } finally {
      if (requestIdRef.current === requestId) setLoading(false);
    }
  }, [client]);

  const reset = useCallback(() => {
    requestIdRef.current++;
    setData(null);
    setLoading(false);
    setError(null);
  }, []);

  return { mutate, data, loading, error, reset };
}

export function useTasksUpdateMutation(client: TasksClient): MutationState<Task> & {
  mutate: (id: number, patch: Patch<Task>) => Promise<Task>;
} {
  const [data, setData] = useState<Task | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<Error | null>(null);
  const requestIdRef = useRef(0);

  const mutate = useCallback(async (id: number, patch: Patch<Task>): Promise<Task> => {
    const requestId = ++requestIdRef.current;
    setLoading(true);
    setError(null);
    try {
      const res = await client.update(id, patch);
      if (requestIdRef.current === requestId) setData(res);
      return res;
    } catch (err) {
      const e = err instanceof Error ? err : new Error(String(err));
      if (requestIdRef.current === requestId) setError(e);
      throw e;
    } finally {
      if (requestIdRef.current === requestId) setLoading(false);
    }
  }, [client]);

  const reset = useCallback(() => {
    requestIdRef.current++;
    setData(null);
    setLoading(false);
    setError(null);
  }, []);

  return { mutate, data, loading, error, reset };
}

export function useTasksRemoveMutation(client: TasksClient): MutationState<boolean> & {
  mutate: (id: number) => Promise<boolean>;
} {
  const [data, setData] = useState<boolean | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<Error | null>(null);
  const requestIdRef = useRef(0);

  const mutate = useCallback(async (id: number): Promise<boolean> => {
    const requestId = ++requestIdRef.current;
    setLoading(true);
    setError(null);
    try {
      const res = await client.remove(id);
      if (requestIdRef.current === requestId) setData(res);
      return res;
    } catch (err) {
      const e = err instanceof Error ? err : new Error(String(err));
      if (requestIdRef.current === requestId) setError(e);
      throw e;
    } finally {
      if (requestIdRef.current === requestId) setLoading(false);
    }
  }, [client]);

  const reset = useCallback(() => {
    requestIdRef.current++;
    setData(null);
    setLoading(false);
    setError(null);
  }, []);

  return { mutate, data, loading, error, reset };
}

export function useTasksListByColumnQuery(client: TasksClient, col: ColumnId, options?: { enabled?: boolean }): QueryState<Task[]> {
  const enabled = options?.enabled ?? true;
  const [data, setData] = useState<Task[] | null>(null);
  const [loading, setLoading] = useState(enabled);
  const [error, setError] = useState<Error | null>(null);
  const requestIdRef = useRef(0);

  const refetch = useCallback(async (): Promise<Task[] | null> => {
    const requestId = ++requestIdRef.current;
    setLoading(true);
    setError(null);
    try {
      const res = await client.listByColumn(col);
      if (requestIdRef.current === requestId) setData(res);
      return res;
    } catch (err) {
      const e = err instanceof Error ? err : new Error(String(err));
      if (requestIdRef.current === requestId) setError(e);
      return null;
    } finally {
      if (requestIdRef.current === requestId) setLoading(false);
    }
  }, [client, col]);

  useEffect(() => {
    if (enabled) {
      refetch();
    }
    return () => { requestIdRef.current++; };
  }, [enabled, refetch]);

  return { data, loading, error, refetch };
}

export function useTasksListByColumnMutation(client: TasksClient): MutationState<Task[]> & {
  mutate: (col: ColumnId) => Promise<Task[]>;
} {
  const [data, setData] = useState<Task[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<Error | null>(null);
  const requestIdRef = useRef(0);

  const mutate = useCallback(async (col: ColumnId): Promise<Task[]> => {
    const requestId = ++requestIdRef.current;
    setLoading(true);
    setError(null);
    try {
      const res = await client.listByColumn(col);
      if (requestIdRef.current === requestId) setData(res);
      return res;
    } catch (err) {
      const e = err instanceof Error ? err : new Error(String(err));
      if (requestIdRef.current === requestId) setError(e);
      throw e;
    } finally {
      if (requestIdRef.current === requestId) setLoading(false);
    }
  }, [client]);

  const reset = useCallback(() => {
    requestIdRef.current++;
    setData(null);
    setLoading(false);
    setError(null);
  }, []);

  return { mutate, data, loading, error, reset };
}

export function useTasksStatsQuery(client: TasksClient, options?: { enabled?: boolean }): QueryState<BoardStats> {
  const enabled = options?.enabled ?? true;
  const [data, setData] = useState<BoardStats | null>(null);
  const [loading, setLoading] = useState(enabled);
  const [error, setError] = useState<Error | null>(null);
  const requestIdRef = useRef(0);

  const refetch = useCallback(async (): Promise<BoardStats | null> => {
    const requestId = ++requestIdRef.current;
    setLoading(true);
    setError(null);
    try {
      const res = await client.stats();
      if (requestIdRef.current === requestId) setData(res);
      return res;
    } catch (err) {
      const e = err instanceof Error ? err : new Error(String(err));
      if (requestIdRef.current === requestId) setError(e);
      return null;
    } finally {
      if (requestIdRef.current === requestId) setLoading(false);
    }
  }, [client]);

  useEffect(() => {
    if (enabled) {
      refetch();
    }
    return () => { requestIdRef.current++; };
  }, [enabled, refetch]);

  return { data, loading, error, refetch };
}

export function useTasksStatsMutation(client: TasksClient): MutationState<BoardStats> & {
  mutate: () => Promise<BoardStats>;
} {
  const [data, setData] = useState<BoardStats | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<Error | null>(null);
  const requestIdRef = useRef(0);

  const mutate = useCallback(async (): Promise<BoardStats> => {
    const requestId = ++requestIdRef.current;
    setLoading(true);
    setError(null);
    try {
      const res = await client.stats();
      if (requestIdRef.current === requestId) setData(res);
      return res;
    } catch (err) {
      const e = err instanceof Error ? err : new Error(String(err));
      if (requestIdRef.current === requestId) setError(e);
      throw e;
    } finally {
      if (requestIdRef.current === requestId) setLoading(false);
    }
  }, [client]);

  const reset = useCallback(() => {
    requestIdRef.current++;
    setData(null);
    setLoading(false);
    setError(null);
  }, []);

  return { mutate, data, loading, error, reset };
}

export function useTasksWatchTasks(client: TasksClient): SubscriptionState<Task> {
  const [data, setData] = useState<Task[]>([]);
  const [latest, setLatest] = useState<Task | null>(null);
  const [isConnected, setIsConnected] = useState(false);
  const [error, setError] = useState<Error | null>(null);

  useEffect(() => {
    let cancelled = false;
    async function run() {
      try {
        setIsConnected(true);
        setError(null);
        for await (const item of client.watchTasks()) {
          if (cancelled) break;
          setLatest(item);
          setData((prev) => [...prev, item]);
        }
      } catch (err) {
        if (!cancelled) setError(err instanceof Error ? err : new Error(String(err)));
      } finally {
        if (!cancelled) setIsConnected(false);
      }
    }
    run();
    return () => { cancelled = true; };
  }, [client]);

  return { data, latest, isConnected, error };
}

