// The bridge to the raylang program. The webview injects `window.ray` into every page it loads
// (the Vite dev server under `ray dev`, the embedded build in a bundle), so this module works the
// same in development and on the phone. In a plain browser there is no program behind the page.

export type Note = {
  id: string
  title: string
  body: string
  updated_ms: number
}

type Answer = { ok: true; notes: Note[] } | { ok: false; error: string }

type RayBridge = { request(value: unknown): Promise<unknown> }

declare global {
  interface Window {
    ray?: RayBridge
  }
}

async function call(request: Record<string, string>): Promise<Note[]> {
  if (!window.ray) {
    throw new Error('No raylang program behind this page: open it with `ray dev`.')
  }
  const answer = (await window.ray.request(request)) as Answer
  if (!answer.ok) {
    throw new Error(answer.error)
  }
  return answer.notes
}

export const listNotes = () => call({ op: 'list' })

export const saveNote = (id: string, title: string, body: string) =>
  call({ op: 'save', id, title, body })

export const deleteNote = (id: string) => call({ op: 'delete', id })
