// Generado automáticamente por linkc v1.97.0 — no editar a mano.

import { useState, useEffect, useCallback, useRef, useSyncExternalStore } from "react";
import type { BoardStats, ColumnId, NewTask, Patch, Task, TasksClient } from "./contract";

export interface QueryState<T> {
  data: T | null;
  loading: boolean;
  isFetching: boolean;
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
  reconnect: () => void;
}

export interface InfiniteQueryState<T> {
  data: T[];
  loading: boolean;
  isFetchingNextPage: boolean;
  hasNextPage: boolean;
  error: Error | null;
  fetchNextPage: () => Promise<void>;
  refetch: () => Promise<void>;
}

type QueryCacheState<T> = { data: T | null; isFetching: boolean; error: Error | null };

type QueryCacheEntry<T> = {
  state: QueryCacheState<T>;
  promise: Promise<T> | null;
  listeners: Set<() => void>;
};

const queryCache = new Map<string, QueryCacheEntry<unknown>>();

function getQueryCacheEntry<T>(key: string): QueryCacheEntry<T> {
  let entry = queryCache.get(key) as QueryCacheEntry<T> | undefined;
  if (!entry) {
    entry = { state: { data: null, isFetching: false, error: null }, promise: null, listeners: new Set() };
    queryCache.set(key, entry as QueryCacheEntry<unknown>);
  }
  return entry;
}

function setQueryCacheState<T>(entry: QueryCacheEntry<T>, patch: Partial<QueryCacheState<T>>): void {
  entry.state = { ...entry.state, ...patch };
  entry.listeners.forEach((listener) => listener());
}

function invalidateQueryCache(rpcKeyPrefix: string): void {
  const prefix = rpcKeyPrefix + "(";
  queryCache.forEach((entry, key) => {
    if (!key.startsWith(prefix)) return;
    entry.state = { data: null, isFetching: false, error: null };
    entry.listeners.forEach((listener) => listener());
  });
}

export function useTasksListQuery(client: TasksClient, options?: { enabled?: boolean }): QueryState<Task[]> {
  const enabled = options?.enabled ?? true;
  const cacheKey = "Tasks.list(" + JSON.stringify([]) + ")";
  const entry = getQueryCacheEntry<Task[]>(cacheKey);

  const subscribe = useCallback((onStoreChange: () => void) => {
    entry.listeners.add(onStoreChange);
    return () => { entry.listeners.delete(onStoreChange); };
  }, [entry]);
  const getSnapshot = useCallback(() => entry.state, [entry]);
  const state = useSyncExternalStore(subscribe, getSnapshot, getSnapshot);

  const refetch = useCallback(async (): Promise<Task[] | null> => {
    if (!entry.promise) {
      setQueryCacheState(entry, { isFetching: true, error: null });
      entry.promise = client.list()
        .then((res) => {
          setQueryCacheState(entry, { data: res, isFetching: false });
          return res;
        })
        .catch((err) => {
          const e = err instanceof Error ? err : new Error(String(err));
          setQueryCacheState(entry, { error: e, isFetching: false });
          throw e;
        })
        .finally(() => {
          entry.promise = null;
        });
    }
    try {
      return await entry.promise;
    } catch {
      return null;
    }
  }, [entry, client]);

  useEffect(() => {
    if (enabled && state.data === null && !state.isFetching && !entry.promise) {
      refetch();
    }
  }, [enabled, refetch, entry, state.data, state.isFetching]);

  return { data: state.data, loading: state.data === null && state.isFetching, isFetching: state.isFetching, error: state.error, refetch };
}

export function useTasksListMutation(client: TasksClient): MutationState<Task[]> & {
  mutate: () => Promise<Task[] | null>;
  mutateAsync: () => Promise<Task[]>;
} {
  const [data, setData] = useState<Task[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<Error | null>(null);
  const requestIdRef = useRef(0);

  const mutateAsync = useCallback(async (): Promise<Task[]> => {
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

  const mutate = useCallback(async (): Promise<Task[] | null> => {
    try {
      return await mutateAsync();
    } catch {
      return null;
    }
  }, [mutateAsync]);

  const reset = useCallback(() => {
    requestIdRef.current++;
    setData(null);
    setLoading(false);
    setError(null);
  }, []);

  return { mutate, mutateAsync, data, loading, error, reset };
}

export function useTasksListPagedInfinite(client: TasksClient, limit: number, options?: { enabled?: boolean }): InfiniteQueryState<Task> {
  const enabled = options?.enabled ?? true;
  const [pages, setPages] = useState<Task[][]>([]);
  const [nextCursor, setNextCursor] = useState<number | null>(null);
  const [hasNextPage, setHasNextPage] = useState(true);
  const [loading, setLoading] = useState(false);
  const [isFetchingNextPage, setIsFetchingNextPage] = useState(false);
  const [error, setError] = useState<Error | null>(null);
  const requestIdRef = useRef(0);
  const startedRef = useRef(false);

  const loadPage = useCallback(async (cursorArg: number | null, replace: boolean): Promise<void> => {
    const requestId = ++requestIdRef.current;
    if (replace) setLoading(true); else setIsFetchingNextPage(true);
    setError(null);
    try {
      const res = await client.listPaged(cursorArg, limit);
      if (requestIdRef.current !== requestId) return;
      setPages((prev) => (replace ? [res] : [...prev, res]));
      setHasNextPage(res.length === limit);
      setNextCursor(res.length > 0 ? res[res.length - 1].id : cursorArg);
    } catch (err) {
      if (requestIdRef.current === requestId) setError(err instanceof Error ? err : new Error(String(err)));
    } finally {
      if (requestIdRef.current === requestId) { if (replace) setLoading(false); else setIsFetchingNextPage(false); }
    }
  }, [client, limit]);

  useEffect(() => {
    if (enabled && !startedRef.current) {
      startedRef.current = true;
      loadPage(null, true);
    }
  }, [enabled, loadPage]);

  const fetchNextPage = useCallback(async (): Promise<void> => {
    if (!hasNextPage || isFetchingNextPage || loading) return;
    await loadPage(nextCursor, false);
  }, [hasNextPage, isFetchingNextPage, loading, nextCursor, loadPage]);

  const refetch = useCallback(async (): Promise<void> => {
    startedRef.current = true;
    setHasNextPage(true);
    await loadPage(null, true);
  }, [loadPage]);

  return { data: pages.flat(), loading, isFetchingNextPage, hasNextPage, error, fetchNextPage, refetch };
}

export function useTasksListPagedMutation(client: TasksClient): MutationState<Task[]> & {
  mutate: (cursor: number | null, limit: number) => Promise<Task[] | null>;
  mutateAsync: (cursor: number | null, limit: number) => Promise<Task[]>;
} {
  const [data, setData] = useState<Task[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<Error | null>(null);
  const requestIdRef = useRef(0);

  const mutateAsync = useCallback(async (cursor: number | null, limit: number): Promise<Task[]> => {
    const requestId = ++requestIdRef.current;
    setLoading(true);
    setError(null);
    try {
      const res = await client.listPaged(cursor, limit);
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

  const mutate = useCallback(async (cursor: number | null, limit: number): Promise<Task[] | null> => {
    try {
      return await mutateAsync(cursor, limit);
    } catch {
      return null;
    }
  }, [mutateAsync]);

  const reset = useCallback(() => {
    requestIdRef.current++;
    setData(null);
    setLoading(false);
    setError(null);
  }, []);

  return { mutate, mutateAsync, data, loading, error, reset };
}

export function useTasksGetByIdQuery(client: TasksClient, id: number, options?: { enabled?: boolean }): QueryState<Task | null> {
  const enabled = options?.enabled ?? true;
  const cacheKey = "Tasks.getById(" + JSON.stringify([id]) + ")";
  const entry = getQueryCacheEntry<Task | null>(cacheKey);

  const subscribe = useCallback((onStoreChange: () => void) => {
    entry.listeners.add(onStoreChange);
    return () => { entry.listeners.delete(onStoreChange); };
  }, [entry]);
  const getSnapshot = useCallback(() => entry.state, [entry]);
  const state = useSyncExternalStore(subscribe, getSnapshot, getSnapshot);

  const refetch = useCallback(async (): Promise<Task | null> => {
    if (!entry.promise) {
      setQueryCacheState(entry, { isFetching: true, error: null });
      entry.promise = client.getById(id)
        .then((res) => {
          setQueryCacheState(entry, { data: res, isFetching: false });
          return res;
        })
        .catch((err) => {
          const e = err instanceof Error ? err : new Error(String(err));
          setQueryCacheState(entry, { error: e, isFetching: false });
          throw e;
        })
        .finally(() => {
          entry.promise = null;
        });
    }
    try {
      return await entry.promise;
    } catch {
      return null;
    }
  }, [entry, client, id]);

  useEffect(() => {
    if (enabled && state.data === null && !state.isFetching && !entry.promise) {
      refetch();
    }
  }, [enabled, refetch, entry, state.data, state.isFetching]);

  return { data: state.data, loading: state.data === null && state.isFetching, isFetching: state.isFetching, error: state.error, refetch };
}

export function useTasksGetByIdMutation(client: TasksClient): MutationState<Task | null> & {
  mutate: (id: number) => Promise<Task | null>;
  mutateAsync: (id: number) => Promise<Task | null>;
} {
  const [data, setData] = useState<Task | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<Error | null>(null);
  const requestIdRef = useRef(0);

  const mutateAsync = useCallback(async (id: number): Promise<Task | null> => {
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

  const mutate = useCallback(async (id: number): Promise<Task | null> => {
    try {
      return await mutateAsync(id);
    } catch {
      return null;
    }
  }, [mutateAsync]);

  const reset = useCallback(() => {
    requestIdRef.current++;
    setData(null);
    setLoading(false);
    setError(null);
  }, []);

  return { mutate, mutateAsync, data, loading, error, reset };
}

export function useTasksCreateMutation(client: TasksClient): MutationState<Task> & {
  mutate: (input: NewTask) => Promise<Task | null>;
  mutateAsync: (input: NewTask) => Promise<Task>;
} {
  const [data, setData] = useState<Task | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<Error | null>(null);
  const requestIdRef = useRef(0);

  const mutateAsync = useCallback(async (input: NewTask): Promise<Task> => {
    const requestId = ++requestIdRef.current;
    setLoading(true);
    setError(null);
    try {
      const res = await client.create(input);
      if (requestIdRef.current === requestId) setData(res);
      invalidateQueryCache("Tasks.list");
      invalidateQueryCache("Tasks.listByColumn");
      invalidateQueryCache("Tasks.stats");
      return res;
    } catch (err) {
      const e = err instanceof Error ? err : new Error(String(err));
      if (requestIdRef.current === requestId) setError(e);
      throw e;
    } finally {
      if (requestIdRef.current === requestId) setLoading(false);
    }
  }, [client]);

  const mutate = useCallback(async (input: NewTask): Promise<Task | null> => {
    try {
      return await mutateAsync(input);
    } catch {
      return null;
    }
  }, [mutateAsync]);

  const reset = useCallback(() => {
    requestIdRef.current++;
    setData(null);
    setLoading(false);
    setError(null);
  }, []);

  return { mutate, mutateAsync, data, loading, error, reset };
}

export function useTasksUpdateMutation(client: TasksClient): MutationState<Task> & {
  mutate: (id: number, patch: Patch<Task>) => Promise<Task | null>;
  mutateAsync: (id: number, patch: Patch<Task>) => Promise<Task>;
} {
  const [data, setData] = useState<Task | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<Error | null>(null);
  const requestIdRef = useRef(0);

  const mutateAsync = useCallback(async (id: number, patch: Patch<Task>): Promise<Task> => {
    const requestId = ++requestIdRef.current;
    setLoading(true);
    setError(null);
    try {
      const res = await client.update(id, patch);
      if (requestIdRef.current === requestId) setData(res);
      invalidateQueryCache("Tasks.list");
      invalidateQueryCache("Tasks.listByColumn");
      invalidateQueryCache("Tasks.stats");
      return res;
    } catch (err) {
      const e = err instanceof Error ? err : new Error(String(err));
      if (requestIdRef.current === requestId) setError(e);
      throw e;
    } finally {
      if (requestIdRef.current === requestId) setLoading(false);
    }
  }, [client]);

  const mutate = useCallback(async (id: number, patch: Patch<Task>): Promise<Task | null> => {
    try {
      return await mutateAsync(id, patch);
    } catch {
      return null;
    }
  }, [mutateAsync]);

  const reset = useCallback(() => {
    requestIdRef.current++;
    setData(null);
    setLoading(false);
    setError(null);
  }, []);

  return { mutate, mutateAsync, data, loading, error, reset };
}

export function useTasksRemoveMutation(client: TasksClient): MutationState<boolean> & {
  mutate: (id: number) => Promise<boolean | null>;
  mutateAsync: (id: number) => Promise<boolean>;
} {
  const [data, setData] = useState<boolean | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<Error | null>(null);
  const requestIdRef = useRef(0);

  const mutateAsync = useCallback(async (id: number): Promise<boolean> => {
    const requestId = ++requestIdRef.current;
    setLoading(true);
    setError(null);
    try {
      const res = await client.remove(id);
      if (requestIdRef.current === requestId) setData(res);
      invalidateQueryCache("Tasks.list");
      invalidateQueryCache("Tasks.listByColumn");
      invalidateQueryCache("Tasks.stats");
      return res;
    } catch (err) {
      const e = err instanceof Error ? err : new Error(String(err));
      if (requestIdRef.current === requestId) setError(e);
      throw e;
    } finally {
      if (requestIdRef.current === requestId) setLoading(false);
    }
  }, [client]);

  const mutate = useCallback(async (id: number): Promise<boolean | null> => {
    try {
      return await mutateAsync(id);
    } catch {
      return null;
    }
  }, [mutateAsync]);

  const reset = useCallback(() => {
    requestIdRef.current++;
    setData(null);
    setLoading(false);
    setError(null);
  }, []);

  return { mutate, mutateAsync, data, loading, error, reset };
}

export function useTasksListByColumnQuery(client: TasksClient, col: ColumnId, options?: { enabled?: boolean }): QueryState<Task[]> {
  const enabled = options?.enabled ?? true;
  const cacheKey = "Tasks.listByColumn(" + JSON.stringify([col]) + ")";
  const entry = getQueryCacheEntry<Task[]>(cacheKey);

  const subscribe = useCallback((onStoreChange: () => void) => {
    entry.listeners.add(onStoreChange);
    return () => { entry.listeners.delete(onStoreChange); };
  }, [entry]);
  const getSnapshot = useCallback(() => entry.state, [entry]);
  const state = useSyncExternalStore(subscribe, getSnapshot, getSnapshot);

  const refetch = useCallback(async (): Promise<Task[] | null> => {
    if (!entry.promise) {
      setQueryCacheState(entry, { isFetching: true, error: null });
      entry.promise = client.listByColumn(col)
        .then((res) => {
          setQueryCacheState(entry, { data: res, isFetching: false });
          return res;
        })
        .catch((err) => {
          const e = err instanceof Error ? err : new Error(String(err));
          setQueryCacheState(entry, { error: e, isFetching: false });
          throw e;
        })
        .finally(() => {
          entry.promise = null;
        });
    }
    try {
      return await entry.promise;
    } catch {
      return null;
    }
  }, [entry, client, col]);

  useEffect(() => {
    if (enabled && state.data === null && !state.isFetching && !entry.promise) {
      refetch();
    }
  }, [enabled, refetch, entry, state.data, state.isFetching]);

  return { data: state.data, loading: state.data === null && state.isFetching, isFetching: state.isFetching, error: state.error, refetch };
}

export function useTasksListByColumnMutation(client: TasksClient): MutationState<Task[]> & {
  mutate: (col: ColumnId) => Promise<Task[] | null>;
  mutateAsync: (col: ColumnId) => Promise<Task[]>;
} {
  const [data, setData] = useState<Task[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<Error | null>(null);
  const requestIdRef = useRef(0);

  const mutateAsync = useCallback(async (col: ColumnId): Promise<Task[]> => {
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

  const mutate = useCallback(async (col: ColumnId): Promise<Task[] | null> => {
    try {
      return await mutateAsync(col);
    } catch {
      return null;
    }
  }, [mutateAsync]);

  const reset = useCallback(() => {
    requestIdRef.current++;
    setData(null);
    setLoading(false);
    setError(null);
  }, []);

  return { mutate, mutateAsync, data, loading, error, reset };
}

export function useTasksStatsQuery(client: TasksClient, options?: { enabled?: boolean }): QueryState<BoardStats> {
  const enabled = options?.enabled ?? true;
  const cacheKey = "Tasks.stats(" + JSON.stringify([]) + ")";
  const entry = getQueryCacheEntry<BoardStats>(cacheKey);

  const subscribe = useCallback((onStoreChange: () => void) => {
    entry.listeners.add(onStoreChange);
    return () => { entry.listeners.delete(onStoreChange); };
  }, [entry]);
  const getSnapshot = useCallback(() => entry.state, [entry]);
  const state = useSyncExternalStore(subscribe, getSnapshot, getSnapshot);

  const refetch = useCallback(async (): Promise<BoardStats | null> => {
    if (!entry.promise) {
      setQueryCacheState(entry, { isFetching: true, error: null });
      entry.promise = client.stats()
        .then((res) => {
          setQueryCacheState(entry, { data: res, isFetching: false });
          return res;
        })
        .catch((err) => {
          const e = err instanceof Error ? err : new Error(String(err));
          setQueryCacheState(entry, { error: e, isFetching: false });
          throw e;
        })
        .finally(() => {
          entry.promise = null;
        });
    }
    try {
      return await entry.promise;
    } catch {
      return null;
    }
  }, [entry, client]);

  useEffect(() => {
    if (enabled && state.data === null && !state.isFetching && !entry.promise) {
      refetch();
    }
  }, [enabled, refetch, entry, state.data, state.isFetching]);

  return { data: state.data, loading: state.data === null && state.isFetching, isFetching: state.isFetching, error: state.error, refetch };
}

export function useTasksStatsMutation(client: TasksClient): MutationState<BoardStats> & {
  mutate: () => Promise<BoardStats | null>;
  mutateAsync: () => Promise<BoardStats>;
} {
  const [data, setData] = useState<BoardStats | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<Error | null>(null);
  const requestIdRef = useRef(0);

  const mutateAsync = useCallback(async (): Promise<BoardStats> => {
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

  const mutate = useCallback(async (): Promise<BoardStats | null> => {
    try {
      return await mutateAsync();
    } catch {
      return null;
    }
  }, [mutateAsync]);

  const reset = useCallback(() => {
    requestIdRef.current++;
    setData(null);
    setLoading(false);
    setError(null);
  }, []);

  return { mutate, mutateAsync, data, loading, error, reset };
}

export function useTasksWatchTasks(client: TasksClient): SubscriptionState<Task> {
  const [data, setData] = useState<Task[]>([]);
  const [latest, setLatest] = useState<Task | null>(null);
  const [isConnected, setIsConnected] = useState(false);
  const [error, setError] = useState<Error | null>(null);
  const [reconnectAttempt, setReconnectAttempt] = useState(0);

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
  }, [client, reconnectAttempt]);

  const reconnect = useCallback(() => {
    setReconnectAttempt((a) => a + 1);
  }, []);

  return { data, latest, isConnected, error, reconnect };
}

