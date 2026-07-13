import { useRef, useState } from 'react'
import type { Rect } from '../api'

interface PxPoint {
  x: number
  y: number
}

interface Props {
  /** 目前頁面渲染 scale（CSS px / pt），與 Viewer 傳給 AnnotLayer 的 scale 一致（不含 devicePixelRatio）。 */
  scale: number
  /** 拖曳出一個有效選取範圍（≥ MIN_PX）後回呼，帶出 view-space points 矩形。 */
  onRectChange: (rectPt: Rect) => void
}

/** 選取矩形小於這個像素門檻視為誤觸，忽略（維持先前選取，不回呼）。 */
const MIN_PX = 8

function pxRect(a: PxPoint, b: PxPoint): Rect {
  return {
    x: Math.min(a.x, b.x),
    y: Math.min(a.y, b.y),
    w: Math.abs(a.x - b.x),
    h: Math.abs(a.y - b.y),
  }
}

export default function CropLayer({ scale, onRectChange }: Props) {
  const overlayRef = useRef<HTMLDivElement>(null)
  const [dragStart, setDragStart] = useState<PxPoint | null>(null)
  const [dragCur, setDragCur] = useState<PxPoint | null>(null)
  const [selPx, setSelPx] = useState<Rect | null>(null)

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
    if (rect.w < MIN_PX || rect.h < MIN_PX) return // 太小：忽略，維持先前選取
    setSelPx(rect)
    onRectChange({ x: rect.x / scale, y: rect.y / scale, w: rect.w / scale, h: rect.h / scale })
  }

  const previewPx = dragStart && dragCur ? pxRect(dragStart, dragCur) : selPx

  return (
    <div
      ref={overlayRef}
      className="crop-layer"
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
    >
      {previewPx && (
        <div
          className="crop-select-rect"
          style={{ left: previewPx.x, top: previewPx.y, width: previewPx.w, height: previewPx.h }}
        />
      )}
    </div>
  )
}
