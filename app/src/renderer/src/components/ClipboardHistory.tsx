import { useCallback, useEffect, useMemo, useRef } from 'react'
import type { ClipboardHistoryDetail, ClipboardHistorySummary, ClipboardKind } from '../api'
import {
  adjacentSelection,
  useClipboardHistoryStore,
  type HistoryError
} from '../clipboardHistoryStore'
import { useI18n } from '../i18n'
import '../assets/clipboardHistory.css'

const PAGE_SIZE = 50
const SEARCH_DEBOUNCE_MS = 150
const REFRESH_DEBOUNCE_MS = 160

function errorState(error: unknown): HistoryError {
  return String(error).includes('history_unavailable') ? 'unavailable' : 'operation'
}

function dataUrl(dataBase64: string): string {
  return `data:image/png;base64,${dataBase64}`
}

function isTextInput(target: EventTarget | null): boolean {
  return (
    target instanceof HTMLInputElement ||
    target instanceof HTMLTextAreaElement ||
    (target instanceof HTMLElement && target.isContentEditable)
  )
}

function HistoryThumbnail({ item }: { item: ClipboardHistorySummary }): React.JSX.Element {
  const ref = useRef<HTMLDivElement>(null)
  const key = `${item.id}:thumb`
  const preview = useClipboardHistoryStore((state) => state.imagePreviews[key])
  const cacheImagePreview = useClipboardHistoryStore((state) => state.cacheImagePreview)

  useEffect(() => {
    if (item.kind !== 'image' || !item.imageAvailable || preview) return
    const element = ref.current
    if (!element) return
    let disposed = false
    const observer = new IntersectionObserver(
      (entries) => {
        if (!entries.some((entry) => entry.isIntersecting)) return
        observer.disconnect()
        void window.api
          .clipboardHistoryImagePreview(item.id, 200, 120)
          .then((result) => {
            if (!disposed && result) cacheImagePreview(key, dataUrl(result.dataBase64))
          })
          .catch(() => undefined)
      },
      { rootMargin: '80px' }
    )
    observer.observe(element)
    return () => {
      disposed = true
      observer.disconnect()
    }
  }, [cacheImagePreview, item.id, item.imageAvailable, item.kind, key, preview])

  if (item.kind === 'image') {
    return (
      <div ref={ref} className="history-row__visual history-row__visual--image">
        {preview ? <img src={preview} alt="" /> : <span>IMG</span>}
      </div>
    )
  }
  const symbol: Record<Exclude<ClipboardKind, 'image'>, string> = {
    text: 'T',
    html: '<>',
    files: 'FILE'
  }
  return (
    <div className={`history-row__visual history-row__visual--${item.kind}`}>
      {symbol[item.kind]}
    </div>
  )
}

function ClipboardHistory(): React.JSX.Element {
  const { language, t } = useI18n()
  const history = t.clipboardHistory
  const items = useClipboardHistoryStore((state) => state.items)
  const selectedId = useClipboardHistoryStore((state) => state.selectedId)
  const query = useClipboardHistoryStore((state) => state.query)
  const loading = useClipboardHistoryStore((state) => state.loading)
  const hasMore = useClipboardHistoryStore((state) => state.hasMore)
  const initialized = useClipboardHistoryStore((state) => state.initialized)
  const visible = useClipboardHistoryStore((state) => state.visible)
  const error = useClipboardHistoryStore((state) => state.error)
  const pasting = useClipboardHistoryStore((state) => state.pasting)
  const feedback = useClipboardHistoryStore((state) => state.feedback)
  const details = useClipboardHistoryStore((state) => state.details)
  const imagePreviews = useClipboardHistoryStore((state) => state.imagePreviews)
  const searchRef = useRef<HTMLInputElement>(null)
  const listRef = useRef<HTMLDivElement>(null)
  const sentinelRef = useRef<HTMLDivElement>(null)
  const rowRefs = useRef(new Map<number, HTMLButtonElement>())
  const requestGeneration = useRef(0)

  const selectedItem = useMemo(
    () => items.find((item) => item.id === selectedId) ?? null,
    [items, selectedId]
  )
  const selectedDetail = selectedId === null ? undefined : details[selectedId]

  const fetchPage = useCallback(async (reset: boolean, queryOverride?: string): Promise<void> => {
    const state = useClipboardHistoryStore.getState()
    if (state.loading && !reset) return
    const generation = reset ? ++requestGeneration.current : requestGeneration.current
    const activeQuery = (queryOverride ?? state.query).trim()
    const offset = reset ? 0 : state.items.length
    state.setLoading(true)
    state.setError(null)
    try {
      const [page, total] = await Promise.all([
        activeQuery
          ? window.api.clipboardHistorySearch(activeQuery, offset, PAGE_SIZE)
          : window.api.clipboardHistoryList(offset, PAGE_SIZE),
        window.api.clipboardHistoryCount(activeQuery || undefined)
      ])
      if (generation !== requestGeneration.current) return
      const more = offset + page.length < total
      if (reset) state.replaceItems(page, more)
      else state.appendItems(page, more)
    } catch (loadError) {
      if (generation === requestGeneration.current) state.setError(errorState(loadError))
    } finally {
      if (generation === requestGeneration.current) state.setLoading(false)
    }
  }, [])

  useEffect(() => {
    void fetchPage(true, '')
  }, [fetchPage])

  useEffect(() => {
    let refreshTimer: number | undefined
    const offChanged = window.api.onClipboardHistoryChanged(() => {
      const state = useClipboardHistoryStore.getState()
      if (!state.visible) {
        state.setDirty(true)
        return
      }
      window.clearTimeout(refreshTimer)
      refreshTimer = window.setTimeout(() => void fetchPage(true), REFRESH_DEBOUNCE_MS)
    })
    const offShown = window.api.onClipboardHistoryWindowShown(() => {
      const state = useClipboardHistoryStore.getState()
      state.setVisible(true)
      state.setDirty(false)
      state.setQuery('')
      window.requestAnimationFrame(() => searchRef.current?.focus())
    })
    const offHidden = window.api.onClipboardHistoryWindowHidden(() => {
      useClipboardHistoryStore.getState().setVisible(false)
    })
    return () => {
      window.clearTimeout(refreshTimer)
      offChanged()
      offShown()
      offHidden()
    }
  }, [fetchPage])

  useEffect(() => {
    if (!visible) return
    const timer = window.setTimeout(() => void fetchPage(true, query), SEARCH_DEBOUNCE_MS)
    return () => window.clearTimeout(timer)
  }, [fetchPage, query, visible])

  useEffect(() => {
    if (selectedId === null || details[selectedId]) return
    let disposed = false
    void window.api
      .clipboardHistoryGet(selectedId)
      .then((detail) => {
        if (!disposed && detail) useClipboardHistoryStore.getState().cacheDetail(detail)
      })
      .catch(() => {
        if (!disposed) useClipboardHistoryStore.getState().setError('operation')
      })
    return () => {
      disposed = true
    }
  }, [details, selectedId])

  useEffect(() => {
    if (!selectedItem || selectedItem.kind !== 'image' || !selectedItem.imageAvailable) return
    const key = `${selectedItem.id}:detail`
    if (imagePreviews[key]) return
    let disposed = false
    void window.api
      .clipboardHistoryImagePreview(selectedItem.id, 1200, 900)
      .then((preview) => {
        if (!disposed && preview) {
          useClipboardHistoryStore.getState().cacheImagePreview(key, dataUrl(preview.dataBase64))
        }
      })
      .catch(() => undefined)
    return () => {
      disposed = true
    }
  }, [imagePreviews, selectedItem])

  useEffect(() => {
    if (selectedId !== null) {
      rowRefs.current.get(selectedId)?.scrollIntoView({ block: 'nearest' })
    }
  }, [selectedId])

  const toggleFavorite = useCallback(async (item: ClipboardHistorySummary): Promise<void> => {
    try {
      const updated = await window.api.clipboardHistorySetFavorite(item.id, !item.isFavorite)
      if (updated) useClipboardHistoryStore.getState().updateFavorite(item.id, !item.isFavorite)
    } catch {
      useClipboardHistoryStore.getState().setError('operation')
    }
  }, [])

  const deleteItem = useCallback(async (id: number): Promise<void> => {
    try {
      if (await window.api.clipboardHistoryDelete(id)) {
        useClipboardHistoryStore.getState().removeItem(id)
      }
    } catch {
      useClipboardHistoryStore.getState().setError('operation')
    }
  }, [])

  const quickPaste = useCallback(async (id: number): Promise<void> => {
    const state = useClipboardHistoryStore.getState()
    if (state.pasting) return
    state.setPasting(true)
    state.setFeedback(null)
    try {
      const outcome = await window.api.quickPaste(id)
      if (outcome.kind === 'copiedOnly') state.setFeedback('copiedOnly')
      if (outcome.kind === 'failed') state.setFeedback('failed')
    } catch {
      state.setFeedback('failed')
    } finally {
      state.setPasting(false)
    }
  }, [])

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent): void => {
      if (event.key === 'Escape') {
        event.preventDefault()
        void window.api.hideClipboardHistory()
        return
      }
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'f') {
        event.preventDefault()
        searchRef.current?.focus()
        searchRef.current?.select()
        return
      }
      if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
        event.preventDefault()
        const state = useClipboardHistoryStore.getState()
        state.setSelectedId(
          adjacentSelection(state.items, state.selectedId, event.key === 'ArrowDown' ? 1 : -1)
        )
        return
      }
      if (event.key === 'Enter' && !event.isComposing) {
        const state = useClipboardHistoryStore.getState()
        const item = state.items.find((candidate) => candidate.id === state.selectedId)
        if (item) {
          event.preventDefault()
          void quickPaste(item.id)
        }
        return
      }
      if (isTextInput(event.target)) return
      const state = useClipboardHistoryStore.getState()
      const item = state.items.find((candidate) => candidate.id === state.selectedId)
      if (!item) return
      if (event.key.toLowerCase() === 'f' && !event.ctrlKey && !event.metaKey && !event.altKey) {
        event.preventDefault()
        void toggleFavorite(item)
      } else if (event.key === 'Delete' || event.key === 'Backspace') {
        event.preventDefault()
        void deleteItem(item.id)
      }
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [deleteItem, quickPaste, toggleFavorite])

  useEffect(() => {
    const root = listRef.current
    const sentinel = sentinelRef.current
    if (!root || !sentinel || !hasMore || loading) return
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) void fetchPage(false)
      },
      { root, rootMargin: '120px' }
    )
    observer.observe(sentinel)
    return () => observer.disconnect()
  }, [fetchPage, hasMore, loading])

  const kindLabel = useCallback(
    (kind: ClipboardKind): string =>
      ({
        text: history.text,
        html: history.html,
        image: history.image,
        files: history.files
      })[kind],
    [history]
  )

  const relativeTime = useCallback(
    (timestamp: number): string => {
      const elapsed = timestamp - Date.now()
      const formatter = new Intl.RelativeTimeFormat(language, { numeric: 'auto' })
      const minute = 60_000
      const hour = 60 * minute
      const day = 24 * hour
      if (Math.abs(elapsed) < minute) return formatter.format(0, 'second')
      if (Math.abs(elapsed) < hour) return formatter.format(Math.round(elapsed / minute), 'minute')
      if (Math.abs(elapsed) < day) return formatter.format(Math.round(elapsed / hour), 'hour')
      return formatter.format(Math.round(elapsed / day), 'day')
    },
    [language]
  )

  const renderPreview = (detail: ClipboardHistoryDetail | undefined): React.JSX.Element => {
    if (!selectedItem) return <div className="history-preview__placeholder">{history.preview}</div>
    if (!detail) return <div className="history-preview__loading" />
    if (detail.kind === 'image') {
      const preview = imagePreviews[`${detail.id}:detail`]
      return preview ? (
        <div className="history-preview__image">
          <img src={preview} alt={history.image} />
        </div>
      ) : (
        <div className="history-preview__placeholder">{history.imageUnavailable}</div>
      )
    }
    if (detail.kind === 'files') {
      return (
        <ul className="history-preview__files">
          {detail.files.map((file) => (
            <li key={file}>{file}</li>
          ))}
        </ul>
      )
    }
    return (
      <pre className="history-preview__text">
        {detail.textContent || selectedItem.previewText || kindLabel(detail.kind)}
      </pre>
    )
  }

  const status = error
    ? error === 'unavailable'
      ? history.unavailable
      : history.operationError
    : query && initialized && items.length === 0
      ? history.noResults
      : initialized && items.length === 0
        ? history.emptyTitle
        : null

  return (
    <main className="clipboard-history">
      <header className="history-header">
        <div className="history-header__title">{history.title}</div>
        <label className="history-search">
          <span aria-hidden="true">⌕</span>
          <input
            ref={searchRef}
            value={query}
            onChange={(event) => useClipboardHistoryStore.getState().setQuery(event.target.value)}
            placeholder={history.searchPlaceholder}
            aria-label={history.searchPlaceholder}
            spellCheck={false}
          />
        </label>
      </header>
      <div className="history-content">
        <section className="history-list" aria-label={history.title}>
          <div ref={listRef} className="history-list__scroll" role="listbox">
            {status ? (
              <div className="history-state">
                <strong>{status}</strong>
                {!error && !query && <span>{history.emptyDescription}</span>}
              </div>
            ) : (
              items.map((item) => (
                <button
                  key={item.id}
                  ref={(element) => {
                    if (element) rowRefs.current.set(item.id, element)
                    else rowRefs.current.delete(item.id)
                  }}
                  type="button"
                  role="option"
                  aria-selected={item.id === selectedId}
                  className={`history-row${item.id === selectedId ? ' is-selected' : ''}`}
                  onClick={() => useClipboardHistoryStore.getState().setSelectedId(item.id)}
                >
                  <HistoryThumbnail item={item} />
                  <span className="history-row__body">
                    <span className="history-row__topline">
                      <span className="history-row__kind">{kindLabel(item.kind)}</span>
                      <time>{relativeTime(item.lastUsedAtMs)}</time>
                    </span>
                    <span className="history-row__preview">
                      {item.previewText || kindLabel(item.kind)}
                    </span>
                    {item.sourceApplication && (
                      <span className="history-row__source">{item.sourceApplication}</span>
                    )}
                  </span>
                  {item.isFavorite && (
                    <span className="history-row__favorite" aria-hidden="true">
                      ★
                    </span>
                  )}
                </button>
              ))
            )}
            {loading && <div className="history-list__loading" />}
            <div ref={sentinelRef} className="history-list__sentinel" />
          </div>
        </section>
        <section className="history-preview">
          <header className="history-preview__header">
            <div>
              <span>{selectedItem ? kindLabel(selectedItem.kind) : history.preview}</span>
              {selectedItem?.kind === 'html' && <small>HTML</small>}
            </div>
            {selectedItem && (
              <div className="history-preview__actions">
                <button
                  type="button"
                  title={selectedItem.isFavorite ? history.unfavorite : history.favorite}
                  aria-label={selectedItem.isFavorite ? history.unfavorite : history.favorite}
                  onClick={() => void toggleFavorite(selectedItem)}
                >
                  {selectedItem.isFavorite ? '★' : '☆'}
                </button>
                <button
                  type="button"
                  className="is-danger"
                  title={history.delete}
                  aria-label={history.delete}
                  onClick={() => void deleteItem(selectedItem.id)}
                >
                  ×
                </button>
              </div>
            )}
          </header>
          <div className="history-preview__content">{renderPreview(selectedDetail)}</div>
          {feedback && <div className="history-feedback">{feedback === 'failed' ? history.pasteFailed : history.pasteCopiedOnly}</div>}
          {selectedDetail?.sourceApplication && (
            <footer className="history-preview__footer">{selectedDetail.sourceApplication}</footer>
          )}
        </section>
      </div>
    </main>
  )
}

export default ClipboardHistory
