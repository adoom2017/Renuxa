'use client';

import { useEffect, useRef, useState } from 'react';

export function useStoredState<T>(key: string, initial: T) {
  const [value, setValue] = useState<T>(initial);
  const [ready, setReady] = useState(false);
  const hydrated = useRef(false);
  useEffect(() => {
    const saved = localStorage.getItem(key);
    queueMicrotask(() => {
      if (saved) { try { setValue(JSON.parse(saved)); } catch {} }
      hydrated.current = true;
      setReady(true);
    });
  }, [key]);
  useEffect(() => { if (hydrated.current) localStorage.setItem(key, JSON.stringify(value)); }, [key, value]);
  return [value, setValue, ready] as const;
}

