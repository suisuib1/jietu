export type QuickPastePlatform = 'windows' | 'macos'

export function quickPastePlatform(): QuickPastePlatform {
  return /Macintosh|Mac OS X/i.test(window.navigator.userAgent) ? 'macos' : 'windows'
}

export function pasteShortcutLabel(platform: QuickPastePlatform = quickPastePlatform()): string {
  return platform === 'macos' ? '⌘V' : 'Ctrl+V'
}

export function historyShortcutLabel(platform: QuickPastePlatform = quickPastePlatform()): string {
  return platform === 'macos' ? '⌥V' : 'Alt+V'
}