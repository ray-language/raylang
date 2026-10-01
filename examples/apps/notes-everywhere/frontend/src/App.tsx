import { useCallback, useEffect, useRef, useState } from 'react'
import {
  Cancelled,
  deleteNote,
  exportNotes,
  hello,
  listNotes,
  saveNote,
  type Hello,
  type Note,
} from './bridge'

type Draft = { id: string; title: string; body: string }

const empty: Draft = { id: '', title: '', body: '' }

function when(ms: number): string {
  return new Date(ms).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' })
}

export default function App() {
  const [env, setEnv] = useState<Hello>({ platform: '', desktop: false })
  const [notes, setNotes] = useState<Note[]>([])
  const [query, setQuery] = useState('')
  const [draft, setDraft] = useState<Draft | null>(null)
  const [armed, setArmed] = useState(false)
  const [status, setStatus] = useState('')
  const [error, setError] = useState('')
  const search = useRef<HTMLInputElement>(null)

  // Every call answers with the list for the current search: one source of truth.
  const run = useCallback(async (op: Promise<Note[]>) => {
    try {
      setNotes(await op)
      setError('')
      return true
    } catch (e) {
      if (!(e instanceof Cancelled)) {
        setError(e instanceof Error ? e.message : String(e))
      }
      return false
    }
  }, [])

  useEffect(() => {
    hello().then(setEnv, () => {})
  }, [])

  useEffect(() => {
    void run(listNotes(query))
  }, [query, run])

  const open = (d: Draft) => {
    setDraft(d)
    setArmed(false)
  }

  const save = async () => {
    if (draft && (await run(saveNote(draft.id, draft.title, draft.body, query)))) {
      setDraft(env.desktop ? draft : null)
      setStatus('Saved')
    }
  }

  const remove = async () => {
    if (!draft?.id) return
    // On the phone the page asks (tap twice); on the desktop the program shows a native dialog.
    if (!env.desktop && !armed) {
      setArmed(true)
      return
    }
    if (await run(deleteNote(draft.id, draft.title, query))) {
      setDraft(null)
    }
  }

  const doExport = useCallback(async () => {
    try {
      const path = await exportNotes()
      if (path) setStatus(`Exported to ${path}`)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }, [])

  // Commands from the native menu (desktop): the program dispatches a `ray-menu` event.
  useEffect(() => {
    const onMenu = (e: Event) => {
      const command = (e as CustomEvent<string>).detail
      if (command === 'new') open(empty)
      if (command === 'find') search.current?.focus()
      if (command === 'export') void doExport()
    }
    window.addEventListener('ray-menu', onMenu)
    return () => window.removeEventListener('ray-menu', onMenu)
  }, [doExport])

  const list = (
    <section className="pane list-pane">
      <header className="bar">
        <h1>Notes</h1>
        <div className="actions">
          {env.desktop && (
            <button className="link" onClick={doExport}>
              Export…
            </button>
          )}
          <button className="link strong" onClick={() => open(empty)}>
            New
          </button>
        </div>
      </header>
      <input
        ref={search}
        className="search"
        type="search"
        placeholder="Search"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
      />
      {notes.length === 0 ? (
        <p className="empty">{query ? 'No notes match.' : 'No notes yet. Tap “New” to write one.'}</p>
      ) : (
        <ul className="list">
          {notes.map((n) => (
            <li key={n.id}>
              <button
                className={draft?.id === n.id ? 'selected' : ''}
                onClick={() => open({ id: n.id, title: n.title, body: n.body })}
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
  )

  const editor = draft && (
    <section className="pane editor-pane">
      <header className="bar">
        <button className="link" onClick={() => setDraft(null)}>
          {env.desktop ? 'Close' : 'Cancel'}
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
          {armed ? 'Tap again to delete' : 'Delete note'}
        </button>
      )}
    </section>
  )

  return (
    <main className={`screen ${env.desktop ? 'desktop' : 'phone'} ${draft ? 'editing' : ''}`}>
      {error && <p className="error">{error}</p>}
      {status && env.desktop && <p className="status">{status}</p>}
      <div className="panes">
        {list}
        {editor ?? (env.desktop && <section className="pane placeholder">Select a note</section>)}
      </div>
    </main>
  )
}
