import { create } from "zustand";
import { devtools } from "zustand/middleware";
import type { NotificationRow } from "@/types/api";

interface NotificationState {
  notifications: NotificationRow[];
  unread_count: number;
  setNotifications: (rows: NotificationRow[], count: number) => void;
  prependNotification: (row: NotificationRow) => void;
  markRead: (id: string) => void;
  markAllRead: () => void;
  clearAll: () => void;
}

export const useNotificationStore = create<NotificationState>()(
  devtools(
    (set) => ({
      notifications: [],
      unread_count: 0,

      setNotifications: (rows, count) =>
        set({ notifications: rows, unread_count: count }, false, "setNotifications"),

      prependNotification: (row) =>
        set(
          (s) => ({
            notifications: [row, ...s.notifications],
            unread_count: s.unread_count + 1,
          }),
          false,
          "prependNotification",
        ),

      markRead: (id) =>
        set(
          (s) => ({
            notifications: s.notifications.map((n) =>
              n.id === id ? { ...n, read: true } : n,
            ),
            unread_count: Math.max(0, s.unread_count - 1),
          }),
          false,
          "markRead",
        ),

      markAllRead: () =>
        set(
          (s) => ({
            notifications: s.notifications.map((n) => ({ ...n, read: true })),
            unread_count: 0,
          }),
          false,
          "markAllRead",
        ),

      clearAll: () =>
        set({ notifications: [], unread_count: 0 }, false, "clearAll"),
    }),
    { name: "NotificationStore", enabled: process.env.NODE_ENV !== "production" },
  ),
);
