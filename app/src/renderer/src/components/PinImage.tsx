import { useEffect, useRef, useState } from 'react'
import { getCurrentWindow, PhysicalSize } from '@tauri-apps/api/window'

const pinId = new URLSearchParams(window.location.search).get('pinId') ?? ''

export default function PinImage(): React.JSX.Element {
  const [imageUrl, setImageUrl] = useState('')
  const [locked, setLocked] = useState(false)
  const [opacity, setOpacity] = useState(1)
  const aspectRatio = useRef(1)
  const correctingSize = useRef(false)
  const userResizing = useRef(false)
  const resizeIdleTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const appWindow = getCurrentWindow()

  useEffect(() => {
    let disposed = false
    if (!pinId) return
    void window.api
      .getPinImage(pinId)
      .then((png) => {
        if (!disposed) setImageUrl(`data:image/png;base64,${png}`)
      })
      .catch(() => undefined)
    void window.api
      .getPinState(pinId)
      .then((state) => {
        if (!disposed) {
          setLocked(state.locked)
          setOpacity(state.opacity)
        }
      })
      .catch(() => undefined)
    return () => {
      disposed = true
    }
  }, [])

  useEffect(() => {
    let disposed = false
    void appWindow
      .onResized(({ payload: size }) => {
        if (
          disposed ||
          locked ||
          !userResizing.current ||
          correctingSize.current ||
          aspectRatio.current <= 0
        )
          return
        if (resizeIdleTimer.current) clearTimeout(resizeIdleTimer.current)
        resizeIdleTimer.current = setTimeout(() => {
          userResizing.current = false
          resizeIdleTimer.current = null
        }, 180)
        const ratio = aspectRatio.current
        const projectedHeight = (ratio * size.width + size.height) / (ratio * ratio + 1)
        const nextHeight = Math.max(projectedHeight, 100, 100 / ratio)
        const nextWidth = ratio * nextHeight
        if (Math.abs(nextWidth - size.width) <= 1 && Math.abs(nextHeight - size.height) <= 1) return
        correctingSize.current = true
        void appWindow
          .setSize(new PhysicalSize(Math.round(nextWidth), Math.round(nextHeight)))
          .finally(() => {
            correctingSize.current = false
          })
      })
      .then((off) => {
        if (disposed) off()
      })
    return () => {
      disposed = true
      if (resizeIdleTimer.current) clearTimeout(resizeIdleTimer.current)
      resizeIdleTimer.current = null
      userResizing.current = false
    }
  }, [appWindow, locked])

  const startDragging = (event: React.PointerEvent<HTMLDivElement>): void => {
    if (locked || event.button !== 0 || (event.target as HTMLElement).closest('button, input'))
      return
    event.preventDefault()
    void appWindow.startDragging()
  }

  const toggleLock = async (): Promise<void> => {
    const next = !locked
    try {
      await window.api.setPinLocked(pinId, next)
      setLocked(next)
    } catch {
      /* keep current state */
    }
  }

  const changeOpacity = async (value: number): Promise<void> => {
    try {
      const next = await window.api.setPinOpacity(pinId, value)
      setOpacity(next)
    } catch {
      /* keep current state */
    }
  }

  return (
    <div
      className={`pin-wrap${locked ? ' is-locked' : ''}`}
      style={{ opacity }}
      onPointerDown={startDragging}
    >
      {imageUrl && (
        <img
          src={imageUrl}
          draggable={false}
          alt="Pinned image"
          onLoad={(event) => {
            aspectRatio.current =
              event.currentTarget.naturalWidth / Math.max(1, event.currentTarget.naturalHeight)
          }}
        />
      )}
      <div className="pin-controls" onPointerDown={(event) => event.stopPropagation()}>
        <button
          type="button"
          onClick={() => void toggleLock()}
          title={locked ? 'Unlock position and size' : 'Lock position and size'}
        >
          {locked ? 'Unlock' : 'Lock'}
        </button>
        <label title="Opacity">
          <span>Opacity</span>
          <input
            type="range"
            min="0.4"
            max="1"
            step="0.05"
            value={opacity}
            onChange={(event) => void changeOpacity(Number(event.target.value))}
          />
        </label>
        <button
          type="button"
          onClick={() => void window.api.copyPinImage(pinId)}
          title="Copy image"
        >
          Copy
        </button>
        <button type="button" onClick={() => void window.api.savePinImage(pinId)} title="Save PNG">
          Save
        </button>
        <button type="button" onClick={() => void window.api.closePinWindow(pinId)} title="Close">
          Close
        </button>
      </div>
      {!locked && (
        <button
          type="button"
          className="pin-resize"
          aria-label="Resize"
          title="Resize"
          onPointerDown={(event) => {
            event.preventDefault()
            event.stopPropagation()
            userResizing.current = true
            if (resizeIdleTimer.current) clearTimeout(resizeIdleTimer.current)
            void appWindow.startResizeDragging('SouthEast').catch(() => {
              userResizing.current = false
            })
          }}
        />
      )}
    </div>
  )
}
