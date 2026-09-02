/**
 * Date-bucketing for the History surface (spec §13: Today / Yesterday /
 * Earlier this week / Older).
 *
 * Buckets by *local calendar day*, not a rolling 24-hour window -- a
 * session at 23:59:59 today and one at 00:00:01 tomorrow must land in
 * different buckets even though they're two seconds apart, and a session
 * from 20 hours ago that crossed midnight must NOT still read as "Today".
 */

export type SessionDateBucket = "today" | "yesterday" | "earlierThisWeek" | "older";

export const SESSION_DATE_BUCKET_ORDER: readonly SessionDateBucket[] = [
  "today",
  "yesterday",
  "earlierThisWeek",
  "older",
];

function startOfLocalDay(date: Date): Date {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate());
}

/**
 * Which bucket `timestamp` (an ISO/RFC3339 string, e.g. `recency_time`)
 * falls into relative to `now`. An unparseable timestamp falls back to
 * "older" rather than throwing -- session data from any provider is
 * best-effort, never guaranteed well-formed.
 */
export function bucketForTimestamp(
  timestamp: string,
  now: Date = new Date()
): SessionDateBucket {
  const date = new Date(timestamp);
  if (Number.isNaN(date.getTime())) {
    return "older";
  }

  const today = startOfLocalDay(now);
  const sessionDay = startOfLocalDay(date);
  const diffDays = Math.round(
    (today.getTime() - sessionDay.getTime()) / (1000 * 60 * 60 * 24)
  );

  // <= 0 covers "today" and any clock-skew/future-dated edge case --
  // never bucket a session as older than it can possibly be.
  if (diffDays <= 0) return "today";
  if (diffDays === 1) return "yesterday";
  if (diffDays <= 7) return "earlierThisWeek";
  return "older";
}

/**
 * Groups items into date buckets, preserving each bucket's incoming
 * relative order (callers sort before bucketing, e.g. by `recency_time`
 * descending) -- this function only partitions, it never re-sorts.
 */
export function bucketSessionsByDate<T>(
  items: T[],
  getTimestamp: (item: T) => string,
  now: Date = new Date()
): Record<SessionDateBucket, T[]> {
  const buckets: Record<SessionDateBucket, T[]> = {
    today: [],
    yesterday: [],
    earlierThisWeek: [],
    older: [],
  };
  for (const item of items) {
    buckets[bucketForTimestamp(getTimestamp(item), now)].push(item);
  }
  return buckets;
}
