import { create } from 'zustand'
import type { AppSettings, ClipboardHistoryTheme } from './api'

export const DEFAULT_CLIPBOARD_HISTORY_THEME: ClipboardHistoryTheme = 'cream-handdrawn'

export function normalizeClipboardHistoryTheme(value: unknown): ClipboardHistoryTheme {
  return value === 'bunny-cloud' ? 'bunny-cloud' : DEFAULT_CLIPBOARD_HISTORY_THEME
}

interface ThemeState {
  theme: ClipboardHistoryTheme
  hydrated: boolean
  saving: boolean
  hydrate: () => Promise<void>
  setTheme: (theme: ClipboardHistoryTheme) => Promise<void>
  syncFromSettings: (settings: AppSettings) => void
}

export const useThemeStore = create<ThemeState>((set, get) => ({
  theme: DEFAULT_CLIPBOARD_HISTORY_THEME,
  hydrated: false,
  saving: false,
  hydrate: async () => {
    if (get().hydrated) return
    try {
      const settings = await window.api.getSettings()
      set({ theme: normalizeClipboardHistoryTheme(settings.theme), hydrated: true })
    } catch {
      set({ theme: DEFAULT_CLIPBOARD_HISTORY_THEME, hydrated: true })
    }
  },
  setTheme: async (theme) => {
    const previous = get().theme
    set({ theme, saving: true })
    try {
      const settings = await window.api.setTheme(theme)
      set({ theme: normalizeClipboardHistoryTheme(settings.theme), hydrated: true, saving: false })
    } catch (error) {
      set({ theme: previous, saving: false })
      throw error
    }
  },
  syncFromSettings: (settings) =>
    set({ theme: normalizeClipboardHistoryTheme(settings.theme), hydrated: true })
}))
