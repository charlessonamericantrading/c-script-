// Generado automáticamente por linkc v1.200.1 — no editar a mano.

import { useState, useEffect, useCallback, useRef } from "react";
import type { Author, BlogClient, Post } from "./contract";

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

export function useBlogCreateAuthorMutation(client: BlogClient): MutationState<Author> & {
  mutate: (name: string, options?: { signal?: AbortSignal; optimisticData?: Author }) => Promise<Author | null>;
  mutateAsync: (name: string, options?: { signal?: AbortSignal; optimisticData?: Author }) => Promise<Author>;
} {
  const [data, setData] = useState<Author | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<Error | null>(null);
  const requestIdRef = useRef(0);

  const mutateAsync = useCallback(async (name: string, options?: { signal?: AbortSignal; optimisticData?: Author }): Promise<Author> => {
    const requestId = ++requestIdRef.current;
    setLoading(true);
    setError(null);
    if (options?.optimisticData !== undefined) setData(options.optimisticData);
    try {
      const res = await client.createAuthor(name, { signal: options?.signal });
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

  const mutate = useCallback(async (name: string, options?: { signal?: AbortSignal; optimisticData?: Author }): Promise<Author | null> => {
    try {
      return await mutateAsync(name, options);
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

export function useBlogCreatePostMutation(client: BlogClient): MutationState<Post> & {
  mutate: (title: string, authorId: number, options?: { signal?: AbortSignal; optimisticData?: Post }) => Promise<Post | null>;
  mutateAsync: (title: string, authorId: number, options?: { signal?: AbortSignal; optimisticData?: Post }) => Promise<Post>;
} {
  const [data, setData] = useState<Post | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<Error | null>(null);
  const requestIdRef = useRef(0);

  const mutateAsync = useCallback(async (title: string, authorId: number, options?: { signal?: AbortSignal; optimisticData?: Post }): Promise<Post> => {
    const requestId = ++requestIdRef.current;
    setLoading(true);
    setError(null);
    if (options?.optimisticData !== undefined) setData(options.optimisticData);
    try {
      const res = await client.createPost(title, authorId, { signal: options?.signal });
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

  const mutate = useCallback(async (title: string, authorId: number, options?: { signal?: AbortSignal; optimisticData?: Post }): Promise<Post | null> => {
    try {
      return await mutateAsync(title, authorId, options);
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

export function useBlogDeleteAuthorMutation(client: BlogClient): MutationState<boolean> & {
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
      const res = await client.deleteAuthor(id, { signal: options?.signal });
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

