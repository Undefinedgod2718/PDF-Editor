import { useState } from 'react'
import { deletePage, insertPage, reorderPages, rotatePage, renderUrl, type DocInfo } from '../api'
import { MergeDialog, ExtractDialog } from './PageDialogs'

interface Props {
  doc: DocInfo
  currentPage: number
  gotoPage: (p: number) => void
  pageVersions: Record<number, number>
  onStructureChanged: () => void | Promise<void>
  onOpenDoc: (id: string) => void | Promise<void>
}

const THUMB_SCALE = 0.25

export default function ThumbnailPanel({
  doc,
  currentPage,
  gotoPage,
  pageVersions,
  onStructureChanged,
  onOpenDoc,
}: Props) {
  const [dragIndex, setDragIndex] = useState<number | null>(null)
  const [overIndex, setOverIndex] = useState<number | null>(null)
  const [busy, setBusy] = useState(false)
  const [showMerge, setShowMerge] = useState(false)
  const [showExtract, setShowExtract] = useState(false)

  const runOp = async (fn: () => Promise<unknown>) => {
    setBusy(true)
    try {
      await fn()
      await onStructureChanged()
    } catch (err) {
      // 與 AnnotLayer/AnnotPanel 一致：Phase 3 尚無全域 toast，失敗只記 console。
      console.error('page operation failed:', err)
    } finally {
      setBusy(false)
    }
  }

  const rotate = (pageIndex: number, current: number) => {
    const next = ((current + 90) % 360) as 0 | 90 | 180 | 270
    void runOp(() => rotatePage(doc.id, pageIndex, next))
  }

  const removePage = (pageIndex: number) => {
    if (doc.pageCount <= 1) return
    void runOp(() => deletePage(doc.id, pageIndex))
  }

  const insertAbove = (pageIndex: number) => {
    void runOp(() => insertPage(doc.id, pageIndex))
  }

  const onDrop = (targetIndex: number) => {
    const from = dragIndex
    setDragIndex(null)
    setOverIndex(null)
    if (from === null || from === targetIndex) return
    const order = doc.pages.map((p) => p.index)
    const [moved] = order.splice(from, 1)
    order.splice(targetIndex, 0, moved)
    void runOp(() => reorderPages(doc.id, order))
  }

  return (
    <div className="thumb-panel">
      <div className="thumb-list">
        {doc.pages.map((page) => (
          <div
            key={page.index}
            className={`thumb ${page.index === currentPage ? 'active' : ''} ${
              overIndex === page.index ? 'thumb-drop-target' : ''
            }`}
            draggable={!busy}
            onClick={() => gotoPage(page.index)}
            onDragStart={() => setDragIndex(page.index)}
            onDragOver={(e) => {
              e.preventDefault()
              setOverIndex(page.index)
            }}
            onDragLeave={() => setOverIndex((v) => (v === page.index ? null : v))}
            onDrop={(e) => {
              e.preventDefault()
              onDrop(page.index)
            }}
            onDragEnd={() => {
              setDragIndex(null)
              setOverIndex(null)
            }}
          >
            <div className="thumb-toolbar">
              <button
                className="tb-btn thumb-tool-btn"
                title="向上插入空白頁"
                onClick={(e) => {
                  e.stopPropagation()
                  insertAbove(page.index)
                }}
              >
                ➕
              </button>
              <button
                className="tb-btn thumb-tool-btn"
                title="旋轉 90°"
                onClick={(e) => {
                  e.stopPropagation()
                  rotate(page.index, page.rotation)
                }}
              >
                ↻
              </button>
              <button
                className="tb-btn thumb-tool-btn"
                title="刪除此頁"
                disabled={doc.pageCount <= 1}
                onClick={(e) => {
                  e.stopPropagation()
                  removePage(page.index)
                }}
              >
                🗑
              </button>
            </div>
            <img
              src={renderUrl(doc.id, page.index, THUMB_SCALE, pageVersions[page.index])}
              width={page.width * THUMB_SCALE}
              height={page.height * THUMB_SCALE}
              loading="lazy"
              alt={`第 ${page.index + 1} 頁`}
              draggable={false}
            />
            <span className="thumb-label">{page.index + 1}</span>
          </div>
        ))}
      </div>
      <div className="thumb-panel-actions">
        <button className="tb-btn" onClick={() => setShowMerge(true)}>
          合併文件
        </button>
        <button className="tb-btn" onClick={() => setShowExtract(true)}>
          擷取頁面
        </button>
      </div>

      {showMerge && (
        <MergeDialog
          doc={doc}
          onClose={() => setShowMerge(false)}
          onOpenDoc={async (id) => {
            setShowMerge(false)
            await onOpenDoc(id)
          }}
        />
      )}
      {showExtract && (
        <ExtractDialog
          doc={doc}
          onClose={() => setShowExtract(false)}
          onOpenDoc={async (id) => {
            setShowExtract(false)
            await onOpenDoc(id)
          }}
        />
      )}
    </div>
  )
}
