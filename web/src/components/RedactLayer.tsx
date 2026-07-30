import { useRef, useState } from 'react'
import type { Rect } from '../api'

interface PxPoint {
  x: number
  y: number
}

interface Props {
  /** 目前頁面渲染 scale（CSS px / pt）。 */
  scale: number
  /** 這一頁已暫存、尚未套用的密文方框（view-space points）。 */
  boxesPt: Rect[]
  /** 拖曳出一個有效範圍（≥ MIN_PX）後回呼，帶出 view-space points 矩形——只新增，不取代既有方框。 */
  onAddBox: (rectPt: Rect) => void
}

/** 拖曳範圍小於這個像素門檻視為誤觸，忽略。 */
const MIN_PX = 8

function pxRect(a: PxPoint, b: PxPoint): Rect {
  return {
    x: Math.min(a.x, b.x),
    y: Math.min(a.y, b.y),
    w: Math.abs(a.x - b.x),
    h: Math.abs(a.y - b.y),
  }
}

export default function RedactLayer({ scale, boxesPt, onAddBox }: Props) {
  const overlayRef = useRef<HTMLDivElement>(null)
  const [dragStart, setDragStart] = useState<PxPoint | null>(null)
  const [dragCur, setDragCur] = useState<PxPoint | null>(null)

  const localPoint = (e: { clientX: number; clientY: number }): PxPoint => {
    const r = overlayRef.current!.getBoundingClientRect()
    return { x: e.clientX - r.left, y: e.clientY - r.top }
  }

  const onPointerDown = (e: React.PointerEvent) => {
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
    if (rect.w < MIN_PX || rect.h < MIN_PX) return // 太小：忽略
    onAddBox({ x: rect.x / scale, y: rect.y / scale, w: rect.w / scale, h: rect.h / scale })
  }

  const dragPreviewPx = dragStart && dragCur ? pxRect(dragStart, dragCur) : null

  return (
    <div
      ref={overlayRef}
      className="redact-layer"
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
    >
      {boxesPt.map((r, i) => (
        <div
          key={i}
          className="redact-pending-rect"
          style={{ left: r.x * scale, top: r.y * scale, width: r.w * scale, height: r.h * scale }}
        />
      ))}
      {dragPreviewPx && (
        <div
          className="redact-pending-rect redact-drag-preview"
          style={{
            left: dragPreviewPx.x,
            top: dragPreviewPx.y,
            width: dragPreviewPx.w,
            height: dragPreviewPx.h,
          }}
        />
      )}
    </div>
  )
}
