export const SCREENSHOT_TIMEOUT = 20000;

/** Bound every asynchronous stage, including browser APIs that may never settle. */
export function waitForScreenshot<T>(work: PromiseLike<T> | T, signal: AbortSignal): Promise<T> {
  return new Promise((resolve, reject) => {
    const abort = () => { signal.removeEventListener("abort", abort); reject(signal.reason); };
    Promise.resolve(work).then(value => {
      signal.removeEventListener("abort", abort);
      resolve(value);
    }, error => {
      signal.removeEventListener("abort", abort);
      reject(error);
    });
    if (signal.aborted) abort();
    else signal.addEventListener("abort", abort, { once: true });
  });
}

export async function screenshotTask<T>(work: (signal: AbortSignal) => Promise<T>, parent?: AbortSignal): Promise<T> {
  const controller = new AbortController();
  const abort = () => controller.abort(parent?.reason);
  if (parent?.aborted) abort();
  else parent?.addEventListener("abort", abort, { once: true });
  const timer = window.setTimeout(() => controller.abort(new Error("screenshot-timeout")), SCREENSHOT_TIMEOUT);
  try {
    controller.signal.throwIfAborted();
    return await work(controller.signal);
  } finally {
    clearTimeout(timer);
    parent?.removeEventListener("abort", abort);
    controller.abort();
  }
}
