# Pad (desktop editor)

A tiny desktop text editor written in raylang, built to show what `std/ui` gives a desktop app:
menus with shortcuts and system roles, native dialogs, a floating panel, the unsaved-changes flow,
the clipboard and the file manager. It is the project of the handbook chapter
[Windows in depth](../../../handbook/windows.en.md).

```sh
ray test          # headless: windows, menus and dialogs without a display
ray dev           # the app, with reload on save
ray bundle        # Pad.app, Pad.exe or the Linux launcher
```
