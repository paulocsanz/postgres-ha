export interface RetryOptions {
  maxAttempts: number;
  delayMs: number;
  backoff?: "linear" | "exponential";
  maxDelayMs?: number;
}

export async function retry<T>(
  fn: () => Promise<T>,
  options: RetryOptions
): Promise<T> {
  const {
    maxAttempts,
    delayMs,
    backoff = "linear",
    maxDelayMs = 30000,
  } = options;
  let lastError: Error | null = null;

  for (let attempt = 1; attempt <= maxAttempts; attempt++) {
    try {
      return await fn();
    } catch (error) {
      lastError = error as Error;
      console.log(
        `Attempt ${attempt}/${maxAttempts} failed: ${lastError.message}`
      );

      if (attempt < maxAttempts) {
        let delay = delayMs;
        if (backoff === "exponential") {
          delay = Math.min(delayMs * Math.pow(2, attempt - 1), maxDelayMs);
        }
        await sleep(delay);
      }
    }
  }

  throw new Error(
    `All ${maxAttempts} attempts failed. Last error: ${lastError?.message}`
  );
}

export async function poll<T>(
  fn: () => Promise<T>,
  condition: (result: T) => boolean,
  timeoutMs: number,
  pollIntervalMs: number = 2000
): Promise<T> {
  const startTime = Date.now();

  while (Date.now() - startTime < timeoutMs) {
    try {
      const result = await fn();
      if (condition(result)) {
        return result;
      }
    } catch (error) {
      console.log(`Poll attempt failed: ${(error as Error).message}`);
    }
    await sleep(pollIntervalMs);
  }

  throw new Error(`Polling timed out after ${timeoutMs}ms`);
}

export function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
