import { useEffect, useRef, useState } from 'react';
export function useRemote<T>(
  identity: string,
  revision: number,
  load: (cancelled: () => boolean) => Promise<T>,
) {
  const current = useRef(load);
  current.current = load;
  const [state, setState] = useState<{
    key: string;
    data: T | null;
    error: unknown;
    loading: boolean;
  }>({ key: '', data: null, error: null, loading: true });
  useEffect(() => {
    let disposed = false;
    setState((previous) => ({
      key: identity,
      data: previous.key === identity ? previous.data : null,
      error: null,
      loading: true,
    }));
    void current
      .current(() => disposed)
      .then(
        (data) => {
          if (!disposed) setState({ key: identity, data, error: null, loading: false });
        },
        (error) => {
          if (!disposed) setState({ key: identity, data: null, error, loading: false });
        },
      );
    return () => {
      disposed = true;
    };
  }, [identity, revision]);
  return state.key === identity ? state : { key: identity, data: null, error: null, loading: true };
}
