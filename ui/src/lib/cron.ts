import type { TranslationKey } from "@/i18n/types";

// ── Validation ───────────────────────────────────────────────────────────────

/** Returns true if the cron expression has exactly 5 whitespace-separated fields. */
export function isValidCron(expr: string): boolean {
  return expr.trim().split(/\s+/).length === 5;
}

// ── Human-readable description ───────────────────────────────────────────────

export function describeCron(
  expr: string,
  t: (key: TranslationKey, values?: Record<string, string | number>) => string,
): string {
  const parts = expr.trim().split(/\s+/);
  if (parts.length < 5) return expr;
  const [min, hour, _dom, _mon, dow] = parts;
  const dayStr =
    dow === "*" ? "" :
    dow === "1-5" ? ` (${t("agents.cron_weekdays")})` :
    ` (${t("agents.cron_days", { dow })})`;
  if (min.startsWith("*/")) {
    const interval = min.slice(2);
    const hourRange = hour === "*" ? "" : ` ${hour}`;
    return t("agents.cron_every_n_min", { interval, hourRange, dayStr });
  }
  if (hour.startsWith("*/")) {
    const interval = hour.slice(2);
    return t("agents.cron_every_n_hours", { interval, min: min.padStart(2, "0"), dayStr });
  }
  if (hour.includes(",")) {
    return t("agents.cron_at_min_hours", { min: min.padStart(2, "0"), hour, dayStr });
  }
  return t("agents.cron_at_time", { hour, min: min.padStart(2, "0"), dayStr });
}

// ── Presets ───────────────────────────────────────────────────────────────────

export interface CronPreset {
  value: string;
  labelKey: TranslationKey;
}

/** Merged presets from both agent heartbeat and cron tasks */
export const CRON_PRESETS: CronPreset[] = [
  { value: "* * * * *", labelKey: "agents.cron_every_minute" },
  { value: "*/2 * * * *", labelKey: "agents.cron_every_2_minutes" },
  { value: "*/5 * * * *", labelKey: "agents.cron_every_5_minutes" },
  { value: "*/10 * * * *", labelKey: "agents.cron_every_10_minutes" },
  { value: "*/15 * * * *", labelKey: "agents.cron_every_15_minutes" },
  { value: "*/30 * * * *", labelKey: "agents.cron_every_30_minutes" },
  { value: "*/30 10-19 * * *", labelKey: "agents.cron_every_30_min_10_19" },
  { value: "0 * * * *", labelKey: "cron.preset_every_hour" },
  { value: "0 10-19 * * *", labelKey: "agents.cron_every_hour_10_19" },
  { value: "0 8-22/2 * * *", labelKey: "agents.cron_every_2_hours_8_22" },
  { value: "0 9,13,18 * * *", labelKey: "agents.cron_3_times_day" },
  { value: "0 10,14,18 * * *", labelKey: "cron.preset_3_times_day" },
  { value: "0 10,14,18,22 * * *", labelKey: "agents.cron_4_times_day" },
  { value: "0 10,12,14,16,20,22 * * *", labelKey: "cron.preset_6_times_day" },
  { value: "0 9 * * *", labelKey: "agents.cron_daily_9" },
  { value: "0 9 * * 1-5", labelKey: "agents.cron_weekdays_9" },
  { value: "0 0 * * 1", labelKey: "cron.preset_weekly_mon" },
  { value: "0 0 1 * *", labelKey: "cron.preset_monthly" },
];

// ── Timezones ────────────────────────────────────────────────────────────────

export interface TimezoneOption {
  value: string;
  labelKey?: TranslationKey;
}

/** Merged timezones: Russia (with translation keys) + global (label = value) */
export const TIMEZONES: TimezoneOption[] = [
  // Russia
  { value: "Europe/Kaliningrad", labelKey: "cron.tz_kaliningrad" },
  { value: "Europe/Moscow", labelKey: "cron.tz_moscow" },
  { value: "Europe/Samara", labelKey: "cron.tz_samara" },
  { value: "Asia/Yekaterinburg", labelKey: "cron.tz_yekaterinburg" },
  { value: "Asia/Omsk", labelKey: "cron.tz_omsk" },
  { value: "Asia/Novosibirsk" },
  { value: "Asia/Krasnoyarsk", labelKey: "cron.tz_krasnoyarsk" },
  { value: "Asia/Irkutsk", labelKey: "cron.tz_irkutsk" },
  { value: "Asia/Yakutsk", labelKey: "cron.tz_yakutsk" },
  { value: "Asia/Vladivostok", labelKey: "cron.tz_vladivostok" },
  { value: "Asia/Magadan", labelKey: "cron.tz_magadan" },
  { value: "Asia/Kamchatka", labelKey: "cron.tz_kamchatka" },
  // Global
  { value: "UTC", labelKey: "cron.tz_utc" },
  { value: "Europe/London" },
  { value: "Europe/Berlin" },
  { value: "Europe/Paris" },
  { value: "America/New_York" },
  { value: "America/Chicago" },
  { value: "America/Denver" },
  { value: "America/Los_Angeles" },
  { value: "Asia/Tokyo" },
  { value: "Asia/Shanghai" },
  { value: "Asia/Kolkata" },
  { value: "Australia/Sydney" },
];
