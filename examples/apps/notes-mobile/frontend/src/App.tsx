import { useEffect, useState } from 'react'
import { deleteNote, listNotes, saveNote, type Note } from './bridge'

type Draft = { id: string; title: string; body: string }

const empty: Draft = { id: '', title: '', body: '' }

function when(ms: number): string {
  return new Date(ms).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' })
}

export default function App() {
  const [notes, setNotes] = useState<Note[]>([])
  const [draft, setDraft] = useState<Draft | null>(null)
  const [error, setError] = useState('')

  // Every call answers with the whole list: one source of truth, no client-side merging.
  const run = async (op: Promise<Note[]>) => {
    try {
      setNotes(await op)
      setError('')
      return true
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
      return false
    }
  }

  useEffect(() => {
    void run(listNotes())
  }, [])

  const save = async () => {
    if (draft && (await run(saveNote(draft.id, draft.title, draft.body)))) {
      setDraft(null)
    }
  }

  const remove = async () => {
    if (draft?.id && (await run(deleteNote(draft.id)))) {
      setDraft(null)
    }
  }

  if (draft) {
    return (
      <main className="screen">
        <header className="bar">
          <button className="link" onClick={() => setDraft(null)}>
            Cancel
          </button>
          <h1>{draft.id ? 'Edit note' : 'New note'}</h1>
          <button className="link strong" onClick={save}>
            Save
          </button>
        </header>
        {error && <p className="error">{error}</p>}
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
      </main>
    )
  }

  return (
    <main className="screen">
      <header className="bar">
        <span />
        <h1>Notes</h1>
        <button className="link strong" onClick={() => setDraft(empty)}>
          New
        </button>
      </header>
      {error && <p className="error">{error}</p>}
      {notes.length === 0 ? (
        <p className="empty">No notes yet. Tap “New” to write the first one.</p>
      ) : (
        <ul className="list">
          {notes.map((n) => (
            <li key={n.id}>
              <button onClick={() => setDraft({ id: n.id, title: n.title, body: n.body })}>
                <strong>{n.title}</strong>
                <span>{n.body.split('\n')[0] || 'No text'}</span>
                <time>{when(n.updated_ms)}</time>
              </button>
            </li>
          ))}
        </ul>
      )}
    </main>
  )
}
