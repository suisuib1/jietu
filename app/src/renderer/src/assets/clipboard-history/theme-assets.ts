import type { ClipboardHistoryTheme } from '../../api'
import creamHanddrawnDecorations from './themes/cream-handdrawn/decorations.svg'
import bunnyCloudDecorations from './themes/bunny-cloud/decorations.svg'

export const themeAssets: Record<ClipboardHistoryTheme, string> = {
  'cream-handdrawn': creamHanddrawnDecorations,
  'bunny-cloud': bunnyCloudDecorations
}
