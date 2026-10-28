/** Coalesce visual updates without rounding or delaying persisted positions. */
export function createReaderProgress(initialFraction: number) {
  let latest = initialFraction;
  let snapshot = initialFraction;
  let timer: number | undefined;
  const listeners = new Set<() => void>();
  function cancel() {
    window.clearTimeout(timer);
    timer = undefined;
  }
  function publish() {
    timer = undefined;
    if (snapshot === latest) return;
    snapshot = latest;
    for (const listener of listeners) listener();
  }
  return {
    getSnapshot: () => snapshot,
    subscribe(listener: () => void) {
      listeners.add(listener);
      return () => { listeners.delete(listener); };
    },
    update(fraction: number) {
      latest = fraction;
      if (timer === undefined && snapshot !== latest) timer = window.setTimeout(publish, 100);
    },
    reset(fraction: number) {
      cancel();
      latest = fraction;
      publish();
    },
    cancel,
  };
}

export type ReaderProgressStore = ReturnType<typeof createReaderProgress>;
