// The bridge to the raylang program. The webview injects `window.ray` into every page it loads
// (the Vite dev server under `ray dev`, the embedded build in a bundle), so this module works the
// same in development, on the desktop and on the phone.

export type Note = {
  id: string
  title: string
  body: string
  updated_ms: number
}

export type Hello = { platform: string; desktop: boolean }

type Answer = { ok: true; notes: Note[] } | { ok: false; error: string }

type RayBridge = { request(value: unknown): Promise<unknown> }

declare global {
  interface Window {
    ray?: RayBridge
  }
}

/** A request the user cancelled (the native "Delete?" dialog): not an error to show. */
export class Cancelled extends Error {}

async function ask<T>(request: Record<string, string>): Promise<T> {
  if (!window.ray) {
    throw new Error('No raylang program behind this page: open it with `ray dev`.')
  }
  return (await window.ray.request(request)) as T
}

async function call(request: Record<string, string>): Promise<Note[]> {
  const answer = await ask<Answer>(request)
  if (!answer.ok) {
    throw answer.error ? new Error(answer.error) : new Cancelled()
  }
  return answer.notes
}

export const hello = () => ask<Hello>({ op: 'hello' })

export const listNotes = (query: string) => call({ op: 'list', query })

export const saveNote = (id: string, title: string, body: string, query: string) =>
  call({ op: 'save', id, title, body, query })

export const deleteNote = (id: string, title: string, query: string) =>
  call({ op: 'delete', id, title, query })

/** Desktop only: the program opens the system save dialog. Resolves with the path, or "". */
export async function exportNotes(): Promise<string> {
  const answer = await ask<{ ok: boolean; exported?: string; error?: string }>({ op: 'export' })
  if (!answer.ok) {
    throw new Error(answer.error)
  }
  return answer.exported ?? ''
}
