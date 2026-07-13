import { useEffect, useState } from 'react'
import { deleteStamp, listStamps, stampImageUrl, uploadStamp, type StampMeta } from '../api'

interface Props {
  selected: StampMeta | null
  onSelect: (s: StampMeta | null) => void
  onClose: () => void
}

export default function StampDrawer({ selected, onSelect, onClose }: Props) {
  const [stamps, setStamps] = useState<StampMeta[]>([])
  const [loading, setLoading] = useState(false)
  const [uploading, setUploading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const refresh = async () => {
    setLoading(true)
    try {
      const res = await listStamps()
      setStamps(res)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    void refresh()
  }, [])

  const handleUpload = async (file: File) => {
    setUploading(true)
    setError(null)
    try {
      await uploadStamp(file)
      await refresh()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setUploading(false)
    }
  }

  const handleDelete = async (id: string) => {
    try {
      await deleteStamp(id)
      if (selected?.id === id) onSelect(null)
      await refresh()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }

  return (
    <div className="search-panel stamp-drawer">
      <div className="search-header">
        <span className="annot-panel-title">印章庫</span>
        <label className="tb-btn" title="上傳印章">
          {uploading ? '…' : '⬆'}
          <input
            type="file"
            accept="image/*"
            hidden
            disabled={uploading}
            onChange={(e) => {
              const f = e.target.files?.[0]
              if (f) void handleUpload(f)
              e.target.value = ''
            }}
          />
        </label>
        <button className="tb-btn" title="關閉" onClick={onClose}>
          ✕
        </button>
      </div>
      {error && <div className="annot-hint stamp-drawer-error">{error}</div>}
      <div className="search-results">
        {loading && <div className="annot-empty">載入中…</div>}
        {!loading && stamps.length === 0 && <div className="annot-empty">尚無印章，請先上傳</div>}
        <div className="stamp-grid">
          {stamps.map((s) => (
            <div
              key={s.id}
              className={`stamp-item ${selected?.id === s.id ? 'active' : ''}`}
              title={s.filename}
              onClick={() => onSelect(s)}
            >
              <img src={stampImageUrl(s.id)} alt={s.filename} draggable={false} />
              <button
                className="tb-btn stamp-item-del"
                title="刪除印章"
                onClick={(e) => {
                  e.stopPropagation()
                  void handleDelete(s.id)
                }}
              >
                🗑
              </button>
            </div>
          ))}
        </div>
      </div>
    </div>
  )
}
