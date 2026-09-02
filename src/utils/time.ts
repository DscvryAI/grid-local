import i18n, { languageLocaleMap } from "../i18n";

// 현재 언어에 따른 로케일 반환
export const getLocale = (language: string): string => {
  return languageLocaleMap[language] || "en-US";
};

export const formatTime = (timestamp: string): string => {
  const date = new Date(timestamp);
  const currentLanguage = i18n.language || "en";
  const locale = getLocale(currentLanguage);

  return date.toLocaleString(locale, {
    year: "numeric",
    month: "long",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
};

export const formatTimeShort = (timestamp: string): string => {
  const date = new Date(timestamp);
  const currentLanguage = i18n.language || "en";
  const locale = getLocale(currentLanguage);

  return date.toLocaleTimeString(locale, {
    hour: "numeric",
    minute: "2-digit",
  });
};

export const formatDateCompact = (timestamp: string): string => {
  const date = new Date(timestamp);
  const currentLanguage = i18n.language || "en";
  const locale = getLocale(currentLanguage);

  return date.toLocaleDateString(locale, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
};

/**
 * Compare whether two timestamps fall on the same calendar day.
 */
export const isSameDay = (a: string, b: string): boolean => {
  const dateA = new Date(a);
  const dateB = new Date(b);
  return (
    dateA.getFullYear() === dateB.getFullYear() &&
    dateA.getMonth() === dateB.getMonth() &&
    dateA.getDate() === dateB.getDate()
  );
};

/**
 * Format a timestamp as a date divider label.
 * Returns "Today", "Yesterday", or a full date like "Friday, June 27, 2025".
 */
export const formatDateDivider = (timestamp: string): string => {
  const date = new Date(timestamp);
  const now = new Date();
  const currentLanguage = i18n.language || "en";
  const locale = getLocale(currentLanguage);

  // Check today
  if (
    date.getFullYear() === now.getFullYear() &&
    date.getMonth() === now.getMonth() &&
    date.getDate() === now.getDate()
  ) {
    return i18n.t("time.today");
  }

  // Check yesterday
  const yesterday = new Date(now);
  yesterday.setDate(yesterday.getDate() - 1);
  if (
    date.getFullYear() === yesterday.getFullYear() &&
    date.getMonth() === yesterday.getMonth() &&
    date.getDate() === yesterday.getDate()
  ) {
    return i18n.t("time.yesterday");
  }

  return date.toLocaleDateString(locale, {
    weekday: "long",
    year: "numeric",
    month: "long",
    day: "numeric",
  });
};

/**
 * Relative "X minutes/hours/days ago" for a timestamp, falling back to a
 * locale-formatted short date beyond a week -- matches
 * `ProjectTree/index.tsx`'s own `formatTimeAgo` and `SessionDateList.tsx`'s
 * former local copy exactly (both mirrored this shape before it was
 * shared). An unparseable timestamp returns the raw string rather than
 * throwing, matching the rest of this module's best-effort convention.
 */
export const formatRelativeTime = (dateStr: string): string => {
  try {
    const date = new Date(dateStr);
    const now = new Date();
    const diffMs = now.getTime() - date.getTime();
    const diffMins = Math.floor(diffMs / (1000 * 60));
    const diffHours = Math.floor(diffMs / (1000 * 60 * 60));
    const diffDays = Math.floor(diffMs / (1000 * 60 * 60 * 24));
    const locale = getLocale(i18n.language || "en");

    if (diffMins < 60) {
      return i18n.t("common.time.minutesAgo", { count: Math.max(diffMins, 0) });
    }
    if (diffHours < 24) {
      return i18n.t("common.time.hoursAgo", { count: diffHours });
    }
    if (diffDays < 7) {
      return i18n.t("common.time.daysAgo", { count: diffDays });
    }
    return date.toLocaleDateString(locale, { month: "short", day: "numeric" });
  } catch {
    return dateStr;
  }
};

export const formatDuration = (minutes: number): string => {
  if (minutes < 1) {
    return i18n.t("time.lessThanMinute");
  }

  const days = Math.floor(minutes / (60 * 24));
  const hours = Math.floor((minutes % (60 * 24)) / 60);
  const mins = Math.floor(minutes % 60);

  const parts: string[] = [];

  if (days > 0) {
    const unit = days === 1 ? i18n.t("time.day") : i18n.t("time.days");
    parts.push(`${days} ${unit}`);
  }

  if (hours > 0) {
    const unit = hours === 1 ? i18n.t("time.hour") : i18n.t("time.hours");
    parts.push(`${hours} ${unit}`);
  }

  if (mins > 0) {
    const unit = mins === 1 ? i18n.t("time.minute") : i18n.t("time.minutes");
    parts.push(`${mins} ${unit}`);
  }

  return parts.join(" ");
};
