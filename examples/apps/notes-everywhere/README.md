# Notes (everywhere)

The notes app from the mobile chapter, now also a desktop app for macOS, Linux and Windows: one
`src/`, one React + TypeScript frontend, five platforms. The data lives in SQLite (the `db`
package) in the folder each platform expects. It is the project of the handbook chapter
[Cross-platform app](../../../handbook/multiplataforma.en.md), which explains it step by step.

```sh
npm --prefix frontend install
ray test                         # the database and the protocol, without a window
ray dev                          # desktop window with hot reload of the interface
ray bundle                       # .app (macOS), .desktop (Linux) or .exe (Windows)
ray bundle --ios                 # Xcode project
ray bundle --android             # Gradle project
```
