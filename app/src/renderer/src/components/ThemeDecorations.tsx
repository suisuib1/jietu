import type { ClipboardHistoryTheme } from '../api'
import { themeAssets } from '../assets/clipboard-history/theme-assets'

interface ThemeDecorationsProps {
  theme: ClipboardHistoryTheme
}

export function ThemeDecorations({ theme }: ThemeDecorationsProps): React.JSX.Element {
  return (
    <div className="history-decorations" aria-hidden="true">
      <img className="history-decorations__art" src={themeAssets[theme]} alt="" />
    </div>
  )
}
