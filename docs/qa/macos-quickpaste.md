# macOS QuickPaste QA Checklist

Run these checks on a real macOS 10.15+ machine. This document records release QA only; it does not grant permissions or implement runtime behavior.

- [ ] Option+V opens Clipboard History.
- [ ] The previously focused application is captured correctly.
- [ ] Enter restores text to the clipboard.
- [ ] The first untrusted QuickPaste shows the Accessibility prompt.
- [ ] Application startup never shows the Accessibility prompt.
- [ ] Without Accessibility, content remains in the clipboard and manual Command+V works.
- [ ] After granting Accessibility, the next QuickPaste auto-pastes.
- [ ] Text paste in Notes.
- [ ] Text paste in VS Code.
- [ ] Text paste in a browser input.
- [ ] Image paste.
- [ ] Multiple files paste in original order.
- [ ] Favorite state is preserved after QuickPaste.
- [ ] QuickPaste does not create a duplicate history item.
- [ ] The used item moves to the top of history.
- [ ] Option+V and History focus-loss/toggle behavior is correct.
- [ ] Retina monitor behavior is correct.
- [ ] External monitor behavior is correct.
- [ ] `alwaysDeny` pasteboard privacy behavior is correct.
- [ ] Behavior remains correct after application restart.

Record the macOS version, architecture, target application, and any failure category without recording clipboard text, HTML, file paths, or image data.