import { describe, expect, it } from "vitest";
import { bucketForTimestamp, bucketSessionsByDate } from "./historyDateBuckets";

// A fixed "now" so every test is deterministic regardless of when it runs.
const NOW = new Date(2026, 5, 15, 12, 0, 0); // Mon Jun 15 2026, 12:00 local

describe("bucketForTimestamp", () => {
  it("buckets a session earlier the same calendar day as today", () => {
    expect(bucketForTimestamp(new Date(2026, 5, 15, 0, 5).toISOString(), NOW)).toBe(
      "today"
    );
  });

  it("buckets a session at 23:59:59 the day before as yesterday, not today", () => {
    expect(
      bucketForTimestamp(new Date(2026, 5, 14, 23, 59, 59).toISOString(), NOW)
    ).toBe("yesterday");
  });

  it("buckets a session at 00:00:01 today as today, not yesterday, even though it's only seconds after midnight", () => {
    expect(bucketForTimestamp(new Date(2026, 5, 15, 0, 0, 1).toISOString(), NOW)).toBe(
      "today"
    );
  });

  it("buckets exactly 2 days ago as earlierThisWeek", () => {
    expect(bucketForTimestamp(new Date(2026, 5, 13, 12, 0).toISOString(), NOW)).toBe(
      "earlierThisWeek"
    );
  });

  it("buckets exactly 7 days ago as earlierThisWeek (inclusive boundary)", () => {
    expect(bucketForTimestamp(new Date(2026, 5, 8, 12, 0).toISOString(), NOW)).toBe(
      "earlierThisWeek"
    );
  });

  it("buckets 8 days ago as older", () => {
    expect(bucketForTimestamp(new Date(2026, 5, 7, 12, 0).toISOString(), NOW)).toBe(
      "older"
    );
  });

  it("never buckets a future-dated or clock-skewed timestamp as anything other than today", () => {
    expect(bucketForTimestamp(new Date(2026, 5, 20, 0, 0).toISOString(), NOW)).toBe(
      "today"
    );
  });

  it("falls back to older for an unparseable timestamp instead of throwing", () => {
    expect(bucketForTimestamp("not a real date", NOW)).toBe("older");
  });
});

describe("bucketSessionsByDate", () => {
  it("groups items into their buckets and preserves incoming order within each bucket", () => {
    const items = [
      { id: "a", ts: new Date(2026, 5, 15, 9, 0).toISOString() }, // today
      { id: "b", ts: new Date(2026, 5, 15, 8, 0).toISOString() }, // today, earlier
      { id: "c", ts: new Date(2026, 5, 14, 10, 0).toISOString() }, // yesterday
      { id: "d", ts: new Date(2026, 5, 1, 10, 0).toISOString() }, // older
    ];

    const buckets = bucketSessionsByDate(items, (item) => item.ts, NOW);

    expect(buckets.today.map((i) => i.id)).toEqual(["a", "b"]);
    expect(buckets.yesterday.map((i) => i.id)).toEqual(["c"]);
    expect(buckets.earlierThisWeek).toEqual([]);
    expect(buckets.older.map((i) => i.id)).toEqual(["d"]);
  });

  it("returns empty arrays for every bucket when given no items", () => {
    const buckets = bucketSessionsByDate<{ ts: string }>([], (item) => item.ts, NOW);
    expect(buckets.today).toEqual([]);
    expect(buckets.yesterday).toEqual([]);
    expect(buckets.earlierThisWeek).toEqual([]);
    expect(buckets.older).toEqual([]);
  });
});
