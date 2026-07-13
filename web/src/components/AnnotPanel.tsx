import { useEffect, useState } from 'react'
import { deleteAnnotation, listAnnotations, type AnnotationInfo, type DocInfo, type Rect } from '../api'

interface Props {
  doc: DocInfo
  currentPage: number
  version: number
  onDeleted: (page: number) => void
  onSelect: (page: number, rect: Rect) => void
  onClose: () => void
}

const TYPE_LABEL: Record<string, string> = {
  Highlight: '螢光標記',
  Underline: '底線',
  StrikeOut: '刪除線',
  Squiggly: '波浪線',
  Text: '便籤',
  Note: '便籤',
  Ink: '手繪',
  FreeText: '文字框',
  // 後端以 PDFium 的 subtype 命名回傳，文字框註解實際落在 "Stamp"（見 wiki 補充說明）
  Stamp: '文字框',
}

export default function AnnotPanel({ doc, currentPage, version, onDeleted, onSelect, onClose }: Props) {
  const [items, setItems] = useState<AnnotationInfo[]>([])
  const [loading, setLoading] = useState(false)

  useEffect(() => {
    let cancelled = false
    setLoading(true)
    listAnnotations(doc.id, currentPage)
      .then((res) => {
        if (!cancelled) setItems(res)
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [doc.id, currentPage, version])

  const remove = async (it: AnnotationInfo) => {
    try {
      // nm 是穩定 ID；舊註解（導入 /NM 前建立）才退回 index。
      await deleteAnnotation(doc.id, currentPage, it.nm ?? String(it.index))
      onDeleted(currentPage)
    } catch (err) {
      console.error('deleteAnnotation failed:', err)
    }
  }

  return (
    <div className="annot-panel">
      <div className="search-header">
        <span className="annot-panel-title">第 {currentPage + 1} 頁註解</span>
        <button className="tb-btn" title="關閉" onClick={onClose}>
          ✕
        </button>
      </div>
      <div className="search-results">
        {loading && <div className="annot-empty">載入中…</div>}
        {!loading && items.length === 0 && <div className="annot-empty">此頁尚無註解</div>}
        {items.map((it) => (
          <div
            key={it.nm ?? it.index}
            className="annot-item"
            onClick={() => it.rect && onSelect(currentPage, it.rect)}
          >
            <div className="annot-item-main">
              <span className="annot-item-type">{TYPE_LABEL[it.type] ?? it.type}</span>
              {it.contents && <span className="annot-item-contents">{it.contents}</span>}
              {it.rect && (
                <span className="annot-item-rect">
                  x:{Math.round(it.rect.x)} y:{Math.round(it.rect.y)} w:{Math.round(it.rect.w)} h:
                  {Math.round(it.rect.h)}
                </span>
              )}
            </div>
            <button
              className="tb-btn annot-item-del"
              title="刪除"
              onClick={(e) => {
                e.stopPropagation()
                void remove(it)
              }}
            >
              🗑
            </button>
          </div>
        ))}
      </div>
    </div>
  )
}
