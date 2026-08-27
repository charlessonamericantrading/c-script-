// Generado automáticamente por linkc v1.125.0 — no editar a mano.

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
  controller: AbortController | null;
};

const queryCache = new WeakMap<object, Map<string, QueryCacheEntry<unknown>>>();

function getQueryCacheEntry<T>(client: object, key: string): QueryCacheEntry<T> {
  let clientCache = queryCache.get(client);
  if (!clientCache) {
    clientCache = new Map();
    queryCache.set(client, clientCache);
  }
  let entry = clientCache.get(key) as QueryCacheEntry<T> | undefined;
  if (!entry) {
    entry = { state: { data: null, isFetching: false, error: null }, promise: null, listeners: new Set(), controller: null };
    clientCache.set(key, entry as QueryCacheEntry<unknown>);
  }
  return entry;
}

function setQueryCacheState<T>(entry: QueryCacheEntry<T>, patch: Partial<QueryCacheState<T>>): void {
  entry.state = { ...entry.state, ...patch };
  entry.listeners.forEach((listener) => listener());
}

function invalidateQueryCache(client: object, rpcKeyPrefix: string): void {
  const clientCache = queryCache.get(client);
  if (!clientCache) return;
  const prefix = rpcKeyPrefix + "(";
  clientCache.forEach((entry, key) => {
    if (!key.startsWith(prefix)) return;
    entry.state = { data: null, isFetching: false, error: null };
    entry.listeners.forEach((listener) => listener());
  });
}

type InfiniteCacheState<T> = { pages: T[][]; nextCursor: number | null; hasNextPage: boolean; loading: boolean; isFetchingNextPage: boolean; error: Error | null };

type InfiniteCacheEntry<T> = {
  state: InfiniteCacheState<T>;
  promise: Promise<void> | null;
  listeners: Set<() => void>;
  controller: AbortController | null;
  started: boolean;
};

const infiniteQueryCache = new WeakMap<object, Map<string, InfiniteCacheEntry<unknown>>>();

function getInfiniteCacheEntry<T>(client: object, key: string): InfiniteCacheEntry<T> {
  let clientCache = infiniteQueryCache.get(client);
  if (!clientCache) {
    clientCache = new Map();
    infiniteQueryCache.set(client, clientCache);
  }
  let entry = clientCache.get(key) as InfiniteCacheEntry<T> | undefined;
  if (!entry) {
    entry = { state: { pages: [], nextCursor: null, hasNextPage: true, loading: false, isFetchingNextPage: false, error: null }, promise: null, listeners: new Set(), controller: null, started: false };
    clientCache.set(key, entry as InfiniteCacheEntry<unknown>);
  }
  return entry;
}

function setInfiniteCacheState<T>(entry: InfiniteCacheEntry<T>, patch: Partial<InfiniteCacheState<T>>): void {
  entry.state = { ...entry.state, ...patch };
  entry.listeners.forEach((listener) => listener());
}

export function useTasksListQuery(client: TasksClient, options?: { enabled?: boolean }): QueryState<Task[]> {
  const enabled = options?.enabled ?? true;
  const cacheKey = "Tasks.list(" + JSON.stringify([]) + ")";
  const entry = getQueryCacheEntry<Task[]>(client, cacheKey);

  const subscribe = useCallback((onStoreChange: () => void) => {
    entry.listeners.add(onStoreChange);
    return () => {
      entry.listeners.delete(onStoreChange);
      if (entry.listeners.size === 0) entry.controller?.abort();
    };
  }, [entry]);
  const getSnapshot = useCallback(() => entry.state, [entry]);
  const state = useSyncExternalStore(subscribe, getSnapshot, getSnapshot);

  const refetch = useCallback(async (): Promise<Task[] | null> => {
    if (!entry.promise) {
      setQueryCacheState(entry, { isFetching: true, error: null });
      const controller = new AbortController();
      entry.controller = controller;
      entry.promise = client.list({ signal: controller.signal })
        .then((res) => {
          setQueryCacheState(entry, { data: res, isFetching: false });
          return res;
        })
        .catch((err) => {
          if (err instanceof DOMException && err.name === "AbortError") {
            setQueryCacheState(entry, { isFetching: false });
            throw err;
          }
          const e = err instanceof Error ? err : new Error(String(err));
          setQueryCacheState(entry, { error: e, isFetching: false });
          throw e;
        })
        .finally(() => {
          entry.promise = null;
          entry.controller = null;
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
  mutate: (options?: { signal?: AbortSignal; optimisticData?: Task[] }) => Promise<Task[] | null>;
  mutateAsync: (options?: { signal?: AbortSignal; optimisticData?: Task[] }) => Promise<Task[]>;
} {
  const [data, setData] = useState<Task[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<Error | null>(null);
  const requestIdRef = useRef(0);

  const mutateAsync = useCallback(async (options?: { signal?: AbortSignal; optimisticData?: Task[] }): Promise<Task[]> => {
    const requestId = ++requestIdRef.current;
    setLoading(true);
    setError(null);
    if (options?.optimisticData !== undefined) setData(options.optimisticData);
    try {
      const res = await client.list({ signal: options?.signal });
      if (requestIdRef.current === requestId) setData(res);
      return res;
    } catch (err) {
      const e = err instanceof Error ? err : new Error(String(err));
      if (requestIdRef.current === requestId) {
        setError(e);
        if (options?.optimisticData !== undefined) setData(null);
      }
      throw e;
    } finally {
      if (requestIdRef.current === requestId) setLoading(false);
    }
  }, [client]);

  const mutate = useCallback(async (options?: { signal?: AbortSignal; optimisticData?: Task[] }): Promise<Task[] | null> => {
    try {
      return await mutateAsync(options);
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
  const cacheKey = "Tasks.listPaged(" + JSON.stringify([limit]) + ")";
  const entry = getInfiniteCacheEntry<Task>(client, cacheKey);

  const subscribe = useCallback((onStoreChange: () => void) => {
    entry.listeners.add(onStoreChange);
    return () => {
      entry.listeners.delete(onStoreChange);
      if (entry.listeners.size === 0) entry.controller?.abort();
    };
  }, [entry]);
  const getSnapshot = useCallback(() => entry.state, [entry]);
  const state = useSyncExternalStore(subscribe, getSnapshot, getSnapshot);

  const loadPage = useCallback(async (cursorArg: number | null, replace: boolean): Promise<void> => {
    if (entry.promise) return;
    setInfiniteCacheState(entry, replace ? { loading: true, error: null } : { isFetchingNextPage: true, error: null });
    const controller = new AbortController();
    entry.controller = controller;
    entry.promise = (async () => {
      try {
        const res = await client.listPaged(cursorArg, limit, { signal: controller.signal });
        setInfiniteCacheState(entry, {
          pages: replace ? [res] : [...entry.state.pages, res],
          hasNextPage: res.length === limit,
          nextCursor: res.length > 0 ? res[res.length - 1].id : cursorArg,
          loading: false,
          isFetchingNextPage: false,
        });
      } catch (err) {
        if (err instanceof DOMException && err.name === "AbortError") {
          setInfiniteCacheState(entry, { loading: false, isFetchingNextPage: false });
          return;
        }
        const e = err instanceof Error ? err : new Error(String(err));
        setInfiniteCacheState(entry, { error: e, loading: false, isFetchingNextPage: false });
      } finally {
        entry.promise = null;
        entry.controller = null;
      }
    })();
    await entry.promise;
  }, [entry, client, limit]);

  useEffect(() => {
    if (enabled && !entry.started && !entry.promise) {
      entry.started = true;
      loadPage(null, true);
    }
  }, [enabled, loadPage, entry]);

  const fetchNextPage = useCallback(async (): Promise<void> => {
    if (!state.hasNextPage || state.isFetchingNextPage || state.loading) return;
    await loadPage(state.nextCursor, false);
  }, [state.hasNextPage, state.isFetchingNextPage, state.loading, state.nextCursor, loadPage]);

  const refetch = useCallback(async (): Promise<void> => {
    entry.started = true;
    setInfiniteCacheState(entry, { hasNextPage: true });
    await loadPage(null, true);
  }, [entry, loadPage]);

  return { data: state.pages.flat(), loading: state.loading, isFetchingNextPage: state.isFetchingNextPage, hasNextPage: state.hasNextPage, error: state.error, fetchNextPage, refetch };
}

export function useTasksListPagedMutation(client: TasksClient): MutationState<Task[]> & {
  mutate: (cursor: number | null, limit: number, options?: { signal?: AbortSignal; optimisticData?: Task[] }) => Promise<Task[] | null>;
  mutateAsync: (cursor: number | null, limit: number, options?: { signal?: AbortSignal; optimisticData?: Task[] }) => Promise<Task[]>;
} {
  const [data, setData] = useState<Task[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<Error | null>(null);
  const requestIdRef = useRef(0);

  const mutateAsync = useCallback(async (cursor: number | null, limit: number, options?: { signal?: AbortSignal; optimisticData?: Task[] }): Promise<Task[]> => {
    const requestId = ++requestIdRef.current;
    setLoading(true);
    setError(null);
    if (options?.optimisticData !== undefined) setData(options.optimisticData);
    try {
      const res = await client.listPaged(cursor, limit, { signal: options?.signal });
      if (requestIdRef.current === requestId) setData(res);
      return res;
    } catch (err) {
      const e = err instanceof Error ? err : new Error(String(err));
      if (requestIdRef.current === requestId) {
        setError(e);
        if (options?.optimisticData !== undefined) setData(null);
      }
      throw e;
    } finally {
      if (requestIdRef.current === requestId) setLoading(false);
    }
  }, [client]);

  const mutate = useCallback(async (cursor: number | null, limit: number, options?: { signal?: AbortSignal; optimisticData?: Task[] }): Promise<Task[] | null> => {
    try {
      return await mutateAsync(cursor, limit, options);
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
  const entry = getQueryCacheEntry<Task | null>(client, cacheKey);

  const subscribe = useCallback((onStoreChange: () => void) => {
    entry.listeners.add(onStoreChange);
    return () => {
      entry.listeners.delete(onStoreChange);
      if (entry.listeners.size === 0) entry.controller?.abort();
    };
  }, [entry]);
  const getSnapshot = useCallback(() => entry.state, [entry]);
  const state = useSyncExternalStore(subscribe, getSnapshot, getSnapshot);

  const refetch = useCallback(async (): Promise<Task | null> => {
    if (!entry.promise) {
      setQueryCacheState(entry, { isFetching: true, error: null });
      const controller = new AbortController();
      entry.controller = controller;
      entry.promise = client.getById(id, { signal: controller.signal })
        .then((res) => {
          setQueryCacheState(entry, { data: res, isFetching: false });
          return res;
        })
        .catch((err) => {
          if (err instanceof DOMException && err.name === "AbortError") {
            setQueryCacheState(entry, { isFetching: false });
            throw err;
          }
          const e = err instanceof Error ? err : new Error(String(err));
          setQueryCacheState(entry, { error: e, isFetching: false });
          throw e;
        })
        .finally(() => {
          entry.promise = null;
          entry.controller = null;
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
  mutate: (id: number, options?: { signal?: AbortSignal; optimisticData?: Task | null }) => Promise<Task | null>;
  mutateAsync: (id: number, options?: { signal?: AbortSignal; optimisticData?: Task | null }) => Promise<Task | null>;
} {
  const [data, setData] = useState<Task | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<Error | null>(null);
  const requestIdRef = useRef(0);

  const mutateAsync = useCallback(async (id: number, options?: { signal?: AbortSignal; optimisticData?: Task | null }): Promise<Task | null> => {
    const requestId = ++requestIdRef.current;
    setLoading(true);
    setError(null);
    if (options?.optimisticData !== undefined) setData(options.optimisticData);
    try {
      const res = await client.getById(id, { signal: options?.signal });
      if (requestIdRef.current === requestId) setData(res);
      return res;
    } catch (err) {
      const e = err instanceof Error ? err : new Error(String(err));
      if (requestIdRef.current === requestId) {
        setError(e);
        if (options?.optimisticData !== undefined) setData(null);
      }
      throw e;
    } finally {
      if (requestIdRef.current === requestId) setLoading(false);
    }
  }, [client]);

  const mutate = useCallback(async (id: number, options?: { signal?: AbortSignal; optimisticData?: Task | null }): Promise<Task | null> => {
    try {
      return await mutateAsync(id, options);
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
  mutate: (input: NewTask, options?: { signal?: AbortSignal; optimisticData?: Task }) => Promise<Task | null>;
  mutateAsync: (input: NewTask, options?: { signal?: AbortSignal; optimisticData?: Task }) => Promise<Task>;
} {
  const [data, setData] = useState<Task | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<Error | null>(null);
  const requestIdRef = useRef(0);

  const mutateAsync = useCallback(async (input: NewTask, options?: { signal?: AbortSignal; optimisticData?: Task }): Promise<Task> => {
    const requestId = ++requestIdRef.current;
    setLoading(true);
    setError(null);
    if (options?.optimisticData !== undefined) setData(options.optimisticData);
    try {
      const res = await client.create(input, { signal: options?.signal });
      if (requestIdRef.current === requestId) setData(res);
      invalidateQueryCache(client, "Tasks.list");
      invalidateQueryCache(client, "Tasks.listByColumn");
      invalidateQueryCache(client, "Tasks.stats");
      return res;
    } catch (err) {
      const e = err instanceof Error ? err : new Error(String(err));
      if (requestIdRef.current === requestId) {
        setError(e);
        if (options?.optimisticData !== undefined) setData(null);
      }
      throw e;
    } finally {
      if (requestIdRef.current === requestId) setLoading(false);
    }
  }, [client]);

  const mutate = useCallback(async (input: NewTask, options?: { signal?: AbortSignal; optimisticData?: Task }): Promise<Task | null> => {
    try {
      return await mutateAsync(input, options);
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
  mutate: (id: number, patch: Patch<Task>, options?: { signal?: AbortSignal; optimisticData?: Task }) => Promise<Task | null>;
  mutateAsync: (id: number, patch: Patch<Task>, options?: { signal?: AbortSignal; optimisticData?: Task }) => Promise<Task>;
} {
  const [data, setData] = useState<Task | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<Error | null>(null);
  const requestIdRef = useRef(0);

  const mutateAsync = useCallback(async (id: number, patch: Patch<Task>, options?: { signal?: AbortSignal; optimisticData?: Task }): Promise<Task> => {
    const requestId = ++requestIdRef.current;
    setLoading(true);
    setError(null);
    if (options?.optimisticData !== undefined) setData(options.optimisticData);
    try {
      const res = await client.update(id, patch, { signal: options?.signal });
      if (requestIdRef.current === requestId) setData(res);
      invalidateQueryCache(client, "Tasks.list");
      invalidateQueryCache(client, "Tasks.listByColumn");
      invalidateQueryCache(client, "Tasks.stats");
      return res;
    } catch (err) {
      const e = err instanceof Error ? err : new Error(String(err));
      if (requestIdRef.current === requestId) {
        setError(e);
        if (options?.optimisticData !== undefined) setData(null);
      }
      throw e;
    } finally {
      if (requestIdRef.current === requestId) setLoading(false);
    }
  }, [client]);

  const mutate = useCallback(async (id: number, patch: Patch<Task>, options?: { signal?: AbortSignal; optimisticData?: Task }): Promise<Task | null> => {
    try {
      return await mutateAsync(id, patch, options);
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
  mutate: (id: number, options?: { signal?: AbortSignal; optimisticData?: boolean }) => Promise<boolean | null>;
  mutateAsync: (id: number, options?: { signal?: AbortSignal; optimisticData?: boolean }) => Promise<boolean>;
} {
  const [data, setData] = useState<boolean | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<Error | null>(null);
  const requestIdRef = useRef(0);

  const mutateAsync = useCallback(async (id: number, options?: { signal?: AbortSignal; optimisticData?: boolean }): Promise<boolean> => {
    const requestId = ++requestIdRef.current;
    setLoading(true);
    setError(null);
    if (options?.optimisticData !== undefined) setData(options.optimisticData);
    try {
      const res = await client.remove(id, { signal: options?.signal });
      if (requestIdRef.current === requestId) setData(res);
      invalidateQueryCache(client, "Tasks.list");
      invalidateQueryCache(client, "Tasks.listByColumn");
      invalidateQueryCache(client, "Tasks.stats");
      return res;
    } catch (err) {
      const e = err instanceof Error ? err : new Error(String(err));
      if (requestIdRef.current === requestId) {
        setError(e);
        if (options?.optimisticData !== undefined) setData(null);
      }
      throw e;
    } finally {
      if (requestIdRef.current === requestId) setLoading(false);
    }
  }, [client]);

  const mutate = useCallback(async (id: number, options?: { signal?: AbortSignal; optimisticData?: boolean }): Promise<boolean | null> => {
    try {
      return await mutateAsync(id, options);
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
  const entry = getQueryCacheEntry<Task[]>(client, cacheKey);

  const subscribe = useCallback((onStoreChange: () => void) => {
    entry.listeners.add(onStoreChange);
    return () => {
      entry.listeners.delete(onStoreChange);
      if (entry.listeners.size === 0) entry.controller?.abort();
    };
  }, [entry]);
  const getSnapshot = useCallback(() => entry.state, [entry]);
  const state = useSyncExternalStore(subscribe, getSnapshot, getSnapshot);

  const refetch = useCallback(async (): Promise<Task[] | null> => {
    if (!entry.promise) {
      setQueryCacheState(entry, { isFetching: true, error: null });
      const controller = new AbortController();
      entry.controller = controller;
      entry.promise = client.listByColumn(col, { signal: controller.signal })
        .then((res) => {
          setQueryCacheState(entry, { data: res, isFetching: false });
          return res;
        })
        .catch((err) => {
          if (err instanceof DOMException && err.name === "AbortError") {
            setQueryCacheState(entry, { isFetching: false });
            throw err;
          }
          const e = err instanceof Error ? err : new Error(String(err));
          setQueryCacheState(entry, { error: e, isFetching: false });
          throw e;
        })
        .finally(() => {
          entry.promise = null;
          entry.controller = null;
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
  mutate: (col: ColumnId, options?: { signal?: AbortSignal; optimisticData?: Task[] }) => Promise<Task[] | null>;
  mutateAsync: (col: ColumnId, options?: { signal?: AbortSignal; optimisticData?: Task[] }) => Promise<Task[]>;
} {
  const [data, setData] = useState<Task[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<Error | null>(null);
  const requestIdRef = useRef(0);

  const mutateAsync = useCallback(async (col: ColumnId, options?: { signal?: AbortSignal; optimisticData?: Task[] }): Promise<Task[]> => {
    const requestId = ++requestIdRef.current;
    setLoading(true);
    setError(null);
    if (options?.optimisticData !== undefined) setData(options.optimisticData);
    try {
      const res = await client.listByColumn(col, { signal: options?.signal });
      if (requestIdRef.current === requestId) setData(res);
      return res;
    } catch (err) {
      const e = err instanceof Error ? err : new Error(String(err));
      if (requestIdRef.current === requestId) {
        setError(e);
        if (options?.optimisticData !== undefined) setData(null);
      }
      throw e;
    } finally {
      if (requestIdRef.current === requestId) setLoading(false);
    }
  }, [client]);

  const mutate = useCallback(async (col: ColumnId, options?: { signal?: AbortSignal; optimisticData?: Task[] }): Promise<Task[] | null> => {
    try {
      return await mutateAsync(col, options);
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
  const entry = getQueryCacheEntry<BoardStats>(client, cacheKey);

  const subscribe = useCallback((onStoreChange: () => void) => {
    entry.listeners.add(onStoreChange);
    return () => {
      entry.listeners.delete(onStoreChange);
      if (entry.listeners.size === 0) entry.controller?.abort();
    };
  }, [entry]);
  const getSnapshot = useCallback(() => entry.state, [entry]);
  const state = useSyncExternalStore(subscribe, getSnapshot, getSnapshot);

  const refetch = useCallback(async (): Promise<BoardStats | null> => {
    if (!entry.promise) {
      setQueryCacheState(entry, { isFetching: true, error: null });
      const controller = new AbortController();
      entry.controller = controller;
      entry.promise = client.stats({ signal: controller.signal })
        .then((res) => {
          setQueryCacheState(entry, { data: res, isFetching: false });
          return res;
        })
        .catch((err) => {
          if (err instanceof DOMException && err.name === "AbortError") {
            setQueryCacheState(entry, { isFetching: false });
            throw err;
          }
          const e = err instanceof Error ? err : new Error(String(err));
          setQueryCacheState(entry, { error: e, isFetching: false });
          throw e;
        })
        .finally(() => {
          entry.promise = null;
          entry.controller = null;
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
  mutate: (options?: { signal?: AbortSignal; optimisticData?: BoardStats }) => Promise<BoardStats | null>;
  mutateAsync: (options?: { signal?: AbortSignal; optimisticData?: BoardStats }) => Promise<BoardStats>;
} {
  const [data, setData] = useState<BoardStats | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<Error | null>(null);
  const requestIdRef = useRef(0);

  const mutateAsync = useCallback(async (options?: { signal?: AbortSignal; optimisticData?: BoardStats }): Promise<BoardStats> => {
    const requestId = ++requestIdRef.current;
    setLoading(true);
    setError(null);
    if (options?.optimisticData !== undefined) setData(options.optimisticData);
    try {
      const res = await client.stats({ signal: options?.signal });
      if (requestIdRef.current === requestId) setData(res);
      return res;
    } catch (err) {
      const e = err instanceof Error ? err : new Error(String(err));
      if (requestIdRef.current === requestId) {
        setError(e);
        if (options?.optimisticData !== undefined) setData(null);
      }
      throw e;
    } finally {
      if (requestIdRef.current === requestId) setLoading(false);
    }
  }, [client]);

  const mutate = useCallback(async (options?: { signal?: AbortSignal; optimisticData?: BoardStats }): Promise<BoardStats | null> => {
    try {
      return await mutateAsync(options);
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

