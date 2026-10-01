import { useCallback, useEffect, useState } from 'react'
import { createNote, deleteNote, listNotes, updateNote, type Note } from './api'

type Draft = { id: string; title: string; body: string }

function when(ms: number): string {
  return new Date(ms).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' })
}

export default function App() {
  const [notes, setNotes] = useState<Note[]>([])
  const [query, setQuery] = useState('')
  const [draft, setDraft] = useState<Draft | null>(null)
  const [error, setError] = useState('')

  const refresh = useCallback(async () => {
    try {
      setNotes(await listNotes(query))
      setError('')
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }, [query])

  useEffect(() => {
    void refresh()
  }, [refresh])

  const act = async (op: () => Promise<unknown>) => {
    try {
      await op()
      setDraft(null)
      await refresh()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }

  const save = () =>
    draft &&
    act(() => (draft.id ? updateNote(draft.id, draft.title, draft.body) : createNote(draft.title, draft.body)))

  const remove = () => draft?.id && confirm(`Delete “${draft.title}”?`) && act(() => deleteNote(draft.id))

  return (
    <main className="screen desktop">
      {error && <p className="error">{error}</p>}
      <div className="panes">
        <section className="pane list-pane">
          <header className="bar">
            <h1>Notes</h1>
            <button className="link strong" onClick={() => setDraft({ id: '', title: '', body: '' })}>
              New
            </button>
          </header>
          <input
            className="search"
            type="search"
            placeholder="Search"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
          {notes.length === 0 ? (
            <p className="empty">{query ? 'No notes match.' : 'No notes yet.'}</p>
          ) : (
            <ul className="list">
              {notes.map((n) => (
                <li key={n.id}>
                  <button
                    className={draft?.id === n.id ? 'selected' : ''}
                    onClick={() => setDraft({ id: n.id, title: n.title, body: n.body })}
                  >
                    <strong>{n.title}</strong>
                    <span>{n.body.split('\n')[0] || 'No text'}</span>
                    <time>{when(n.updated_ms)}</time>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </section>
        {draft ? (
          <section className="pane editor-pane">
            <header className="bar">
              <button className="link" onClick={() => setDraft(null)}>
                Close
              </button>
              <h1>{draft.id ? 'Edit note' : 'New note'}</h1>
              <button className="link strong" onClick={save}>
                Save
              </button>
            </header>
            <input
              className="title"
              placeholder="Title"
              value={draft.title}
              autoFocus
              onChange={(e) => setDraft({ ...draft, title: e.target.value })}
            />
            <textarea
              className="body"
              placeholder="Write something…"
              value={draft.body}
              onChange={(e) => setDraft({ ...draft, body: e.target.value })}
            />
            {draft.id && (
              <button className="danger" onClick={remove}>
                Delete note
              </button>
            )}
          </section>
        ) : (
          <section className="pane placeholder">Select a note</section>
        )}
      </div>
    </main>
  )
}
