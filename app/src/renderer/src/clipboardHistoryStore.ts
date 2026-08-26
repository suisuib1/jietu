import { create } from 'zustand'
import type { ClipboardHistoryDetail, ClipboardHistorySummary } from './api'

export type HistoryError = 'unavailable' | 'operation' | null

interface ClipboardHistoryState {
  items: ClipboardHistorySummary[]
  selectedId: number | null
  query: string
  loading: boolean
  hasMore: boolean
  initialized: boolean
  visible: boolean
  dirty: boolean
  error: HistoryError
  pasting: boolean
  feedback: 'copiedOnly' | 'accessibilityRequired' | 'failed' | null
  details: Record<number, ClipboardHistoryDetail>
  imagePreviews: Record<string, string>
  replaceItems: (items: ClipboardHistorySummary[], hasMore: boolean) => void
  appendItems: (items: ClipboardHistorySummary[], hasMore: boolean) => void
  setSelectedId: (id: number | null) => void
  setQuery: (query: string) => void
  setLoading: (loading: boolean) => void
  setVisible: (visible: boolean) => void
  setDirty: (dirty: boolean) => void
  setError: (error: HistoryError) => void
  setPasting: (pasting: boolean) => void
  setFeedback: (feedback: 'copiedOnly' | 'accessibilityRequired' | 'failed' | null) => void
  cacheDetail: (detail: ClipboardHistoryDetail) => void
  cacheImagePreview: (key: string, dataUrl: string) => void
  updateFavorite: (id: number, favorite: boolean) => void
  removeItem: (id: number) => void
}

const MAX_DETAIL_CACHE = 20
const MAX_IMAGE_CACHE = 30

function limitedRecord<T>(
  record: Record<string, T>,
  key: string,
  value: T,
  limit: number
): Record<string, T> {
  const next = { ...record, [key]: value }
  const keys = Object.keys(next)
  if (keys.length > limit) delete next[keys[0]]
  return next
}

export function mergeHistoryPages(
  current: ClipboardHistorySummary[],
  incoming: ClipboardHistorySummary[]
): ClipboardHistorySummary[] {
  const seen = new Set<number>()
  return [...current, ...incoming].filter((item) => {
    if (seen.has(item.id)) return false
    seen.add(item.id)
    return true
  })
}

export function adjacentSelection(
  items: ClipboardHistorySummary[],
  selectedId: number | null,
  direction: -1 | 1
): number | null {
  if (items.length === 0) return null
  const current = items.findIndex((item) => item.id === selectedId)
  const index = current < 0 ? 0 : Math.max(0, Math.min(items.length - 1, current + direction))
  return items[index].id
}

export const useClipboardHistoryStore = create<ClipboardHistoryState>((set) => ({
  items: [],
  selectedId: null,
  query: '',
  loading: false,
  hasMore: false,
  initialized: false,
  visible: false,
  dirty: false,
  error: null,
  pasting: false,
  feedback: null,
  details: {},
  imagePreviews: {},
  replaceItems: (items, hasMore) =>
    set((state) => ({
      items,
      hasMore,
      initialized: true,
      selectedId: items.some((item) => item.id === state.selectedId)
        ? state.selectedId
        : (items[0]?.id ?? null)
    })),
  appendItems: (items, hasMore) =>
    set((state) => ({
      items: mergeHistoryPages(state.items, items),
      hasMore
    })),
  setSelectedId: (selectedId) => set({ selectedId }),
  setQuery: (query) => set({ query }),
  setLoading: (loading) => set({ loading }),
  setVisible: (visible) => set({ visible }),
  setDirty: (dirty) => set({ dirty }),
  setError: (error) => set({ error, initialized: true }),
  setPasting: (pasting) => set({ pasting }),
  setFeedback: (feedback) => set({ feedback }),
  cacheDetail: (detail) =>
    set((state) => ({
      details: limitedRecord(state.details, String(detail.id), detail, MAX_DETAIL_CACHE)
    })),
  cacheImagePreview: (key, dataUrl) =>
    set((state) => ({
      imagePreviews: limitedRecord(state.imagePreviews, key, dataUrl, MAX_IMAGE_CACHE)
    })),
  updateFavorite: (id, favorite) =>
    set((state) => ({
      items: state.items.map((item) => (item.id === id ? { ...item, isFavorite: favorite } : item)),
      details: state.details[id]
        ? {
            ...state.details,
            [id]: { ...state.details[id], isFavorite: favorite }
          }
        : state.details
    })),
  removeItem: (id) =>
    set((state) => {
      const index = state.items.findIndex((item) => item.id === id)
      const items = state.items.filter((item) => item.id !== id)
      const nextIndex = Math.min(Math.max(index, 0), items.length - 1)
      const details = { ...state.details }
      delete details[id]
      return {
        items,
        details,
        selectedId: items[nextIndex]?.id ?? null
      }
    })
}))
