import { useEffect, useRef, useState } from 'react'
import { searchDoc, type DocInfo, type SearchHit } from '../api'

interface Props {
  doc: DocInfo
  hits: SearchHit[]
  setHits: (h: SearchHit[]) => void
  activeHit: number
  gotoHit: (i: number) => void
  onClose: () => void
}

export default function SearchPanel({
  doc,
  hits,
  setHits,
  activeHit,
  gotoHit,
  onClose,
}: Props) {
  const [query, setQuery] = useState('')
  const [searching, setSearching] = useState(false)
  const inputRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    inputRef.current?.focus()
  }, [])

  const run = async () => {
    const q = query.trim()
    if (!q) return
    setSearching(true)
    try {
      const result = await searchDoc(doc.id, q)
      setHits(result)
      if (result.length > 0) gotoHit(0)
    } finally {
      setSearching(false)
    }
  }

  return (
    <div className="search-panel">
      <div className="search-header">
        <input
          ref={inputRef}
          value={query}
          placeholder="搜尋文件…"
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') void run()
            if (e.key === 'Escape') onClose()
          }}
        />
        <button className="tb-btn" onClick={() => void run()} disabled={searching}>
          {searching ? '…' : '🔍'}
        </button>
        <button className="tb-btn" title="關閉" onClick={onClose}>
          ✕
        </button>
      </div>
      <div className="search-nav">
        <span>{hits.length} 筆結果</span>
        <button
          className="tb-btn"
          disabled={hits.length === 0}
          onClick={() => gotoHit((activeHit - 1 + hits.length) % hits.length)}
        >
          ▲
        </button>
        <button
          className="tb-btn"
          disabled={hits.length === 0}
          onClick={() => gotoHit((activeHit + 1) % hits.length)}
        >
          ▼
        </button>
      </div>
      <div className="search-results">
        {hits.map((h, i) => (
          <div
            key={i}
            className={`search-result ${i === activeHit ? 'active' : ''}`}
            onClick={() => gotoHit(i)}
          >
            <span className="sr-page">第 {h.page + 1} 頁</span>
            <span className="sr-excerpt">{h.excerpt}</span>
          </div>
        ))}
      </div>
    </div>
  )
}
