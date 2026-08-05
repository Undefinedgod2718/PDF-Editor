import { useCallback, useEffect, useRef, useState } from 'react'
import { deleteLink, listLinks, type LinkInfo, type Rect } from '../api'

interface PxPoint {
  x: number
  y: number
}

interface Props {
  docId: string
  page: number
  scale: number
  /** 該頁註解版本；連結增刪後由上層 bump，觸發重新載入。 */
  version: number
  /** 拖出一個有效範圍後回呼（view-space points），由上層開對話框問目標。 */
  onCreateRect: (rectPt: Rect) => void
  /** 刪除成功後通知上層 bump 該頁版本。 */
  onChanged: () => void
}

/** 拖曳範圍小於這個像素門檻視為誤觸。後端另有 4pt 的下限。 */
const MIN_PX = 8

function pxRect(a: PxPoint, b: PxPoint): Rect {
  return {
    x: Math.min(a.x, b.x),
    y: Math.min(a.y, b.y),
    w: Math.abs(a.x - b.x),
    h: Math.abs(a.y - b.y),
  }
}

function describe(link: LinkInfo): string {
  if (link.target === 'page') return `→ 第 ${link.page + 1} 頁`
  if (link.target === 'uri') return link.url
  return '（其他動作）'
}

export default function LinkLayer({ docId, page, scale, version, onCreateRect, onChanged }: Props) {
  const overlayRef = useRef<HTMLDivElement>(null)
  const [links, setLinks] = useState<LinkInfo[]>([])
  const [dragStart, setDragStart] = useState<PxPoint | null>(null)
  const [dragCur, setDragCur] = useState<PxPoint | null>(null)

  const load = useCallback(async () => {
    try {
      setLinks(await listLinks(docId, page))
    } catch {
      // 連結列表載不到不該擋住其他編輯動作；框就先不顯示。
      setLinks([])
    }
  }, [docId, page])

  useEffect(() => {
    void load()
  }, [load, version])

  const localPoint = (e: { clientX: number; clientY: number }): PxPoint => {
    const r = overlayRef.current!.getBoundingClientRect()
    return { x: e.clientX - r.left, y: e.clientY - r.top }
  }

  const onPointerDown = (e: React.PointerEvent) => {
    // 點在既有連結的刪除鈕上時不要開始拉框。
    if ((e.target as HTMLElement).closest('.link-delete')) return
    overlayRef.current?.setPointerCapture(e.pointerId)
    const p = localPoint(e)
    setDragStart(p)
    setDragCur(p)
  }

  const onPointerMove = (e: React.PointerEvent) => {
    if (!dragStart) return
    setDragCur(localPoint(e))
  }

  const onPointerUp = (e: React.PointerEvent) => {
    try {
      overlayRef.current?.releasePointerCapture(e.pointerId)
    } catch {
      /* pointer capture already released */
    }
    if (!dragStart || !dragCur) return
    const rect = pxRect(dragStart, dragCur)
    setDragStart(null)
    setDragCur(null)
    if (rect.w < MIN_PX || rect.h < MIN_PX) return
    onCreateRect({ x: rect.x / scale, y: rect.y / scale, w: rect.w / scale, h: rect.h / scale })
  }

  const remove = async (index: number) => {
    try {
      await deleteLink(docId, page, index)
      await load()
      onChanged()
    } catch (e) {
      window.alert(e instanceof Error ? e.message : String(e))
    }
  }

  const preview = dragStart && dragCur ? pxRect(dragStart, dragCur) : null

  return (
    <div
      ref={overlayRef}
      className="link-layer"
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
    >
      {links.map((link) => (
        <div
          key={link.index}
          className="link-rect"
          style={{
            left: link.rect.x * scale,
            top: link.rect.y * scale,
            width: link.rect.w * scale,
            height: link.rect.h * scale,
          }}
          title={describe(link)}
        >
          <span className="link-label">{describe(link)}</span>
          <button
            className="link-delete"
            title="刪除這個連結"
            onClick={() => void remove(link.index)}
          >
            ✕
          </button>
        </div>
      ))}
      {preview && (
        <div
          className="link-rect link-drag-preview"
          style={{ left: preview.x, top: preview.y, width: preview.w, height: preview.h }}
        />
      )}
    </div>
  )
}
