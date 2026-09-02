/**
 * Cold-start timing: measures real wall-clock time from `main.tsx`'s
 * first line to the first real content paint (the moment `AppLayout`
 * stops rendering its full-screen "Building your Grid" loading state), so
 * lazy-loading changes elsewhere can be judged against an actual
 * before/after number instead of assumed to help.
 */

const START_MARK = "grid-local:cold-start-begin";
const READY_MARK = "grid-local:cold-start-ready";
const MEASURE_NAME = "grid-local:cold-start";

export function markColdStartBegin(): void {
  performance.mark(START_MARK);
}

/** Call once, the first time the app has something real to show. */
export function markColdStartReady(): void {
  if (performance.getEntriesByName(READY_MARK).length > 0) {
    return; // already marked -- only the first paint counts
  }
  performance.mark(READY_MARK);
  try {
    const measure = performance.measure(MEASURE_NAME, START_MARK, READY_MARK);
    console.log(`[cold-start] ready in ${measure.duration.toFixed(0)}ms`);
  } catch {
    // START_MARK missing (e.g. a hot-reload during dev) -- not worth
    // surfacing as an error, just skip the measurement.
  }
}
