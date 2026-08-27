import type { ClipboardHistoryTheme } from '../api'
import { useI18n } from '../i18n'

interface ThemePickerProps {
  theme: ClipboardHistoryTheme
  saving: boolean
  onChange: (theme: ClipboardHistoryTheme) => void
}

export function ThemePicker({ theme, saving, onChange }: ThemePickerProps): React.JSX.Element {
  const { t } = useI18n()
  const labels = t.clipboardHistory
  const options: Array<{ id: ClipboardHistoryTheme; label: string }> = [
    { id: 'cream-handdrawn', label: labels.creamHanddrawn },
    { id: 'bunny-cloud', label: labels.bunnyCloud }
  ]

  return (
    <div className="history-theme-picker" role="group" aria-label={labels.themePickerLabel}>
      {options.map((option) => (
        <button
          key={option.id}
          type="button"
          className={`history-theme-picker__option${theme === option.id ? ' is-selected' : ''}`}
          aria-label={option.label}
          aria-pressed={theme === option.id}
          disabled={saving}
          onClick={() => onChange(option.id)}
        >
          <span
            className={`history-theme-picker__swatch history-theme-picker__swatch--${option.id}`}
          />
          <span>{option.label}</span>
        </button>
      ))}
    </div>
  )
}
