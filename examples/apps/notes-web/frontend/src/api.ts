// The JSON API of the raylang program. The same relative URLs work in development (Vite forwards
// /api) and in production (the program serves the page and the API from one origin).

export type Note = { id: string; title: string; body: string; updated_ms: number }

async function call<T>(method: string, url: string, body?: unknown): Promise<T> {
  const res = await fetch(url, {
    method,
    headers: body === undefined ? {} : { 'content-type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
  })
  if (!res.ok) {
    const err = (await res.json().catch(() => ({}))) as { error?: string }
    throw new Error(err.error ?? `HTTP ${res.status}`)
  }
  return (res.status === 204 ? undefined : await res.json()) as T
}

export const listNotes = (q: string) => call<Note[]>('GET', `/api/notes?q=${encodeURIComponent(q)}`)
export const createNote = (title: string, body: string) => call<Note>('POST', '/api/notes', { title, body })
export const updateNote = (id: string, title: string, body: string) =>
  call<Note>('PUT', `/api/notes/${id}`, { title, body })
export const deleteNote = (id: string) => call<void>('DELETE', `/api/notes/${id}`)
