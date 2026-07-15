import { useEffect, useRef, useState } from 'react'
import { listPageImages, type ImageInfo, type Rect } from '../api'

interface PxPoint {
  x: number
  y: number
}

interface Props {
  docId: string
  page: number
  scale: number
  /** 該頁目前版本號（插入/取代影像後會 +1），用來觸發重新抓取影像清單。 */
  version: number
  /** 頁面尺寸（view-space points，與渲染畫面一致），供點擊放置時夾限範圍。 */
  pageWidth: number
  pageHeight: number
  /** 目前選取的既有影像 index（由上層控制，取代影像用）。 */
  selectedIndex: number | null
  onSelectImage: (img: ImageInfo) => void
  /** 是否已選好要插入的檔案，等待在頁面上拖曳/點擊放置。 */
  insertArmed: boolean
  /** 插入檔案的原始尺寸換算成 points（96dpi），供點擊放置（未拖曳）時使用。 */
  insertNaturalPt: { w: number; h: number } | null
  /** 拖曳或點擊放置完成後回呼，帶出 view-space points 矩形。 */
  onInsertRectChange: (rectPt: Rect) => void
}

/** 拖曳矩形雙軸皆小於這個 points 門檻視為點擊放置，改用原始尺寸。 */
const CLICK_PT = 5

function pxRect(a: PxPoint, b: PxPoint): Rect {
  return {
    x: Math.min(a.x, b.x),
    y: Math.min(a.y, b.y),
    w: Math.abs(a.x - b.x),
    h: Math.abs(a.y - b.y),
  }
}

export default function ImageLayer({
  docId,
  page,
  scale,
  version,
  pageWidth,
  pageHeight,
  selectedIndex,
  onSelectImage,
  insertArmed,
  insertNaturalPt,
  onInsertRectChange,
}: Props) {
  const overlayRef = useRef<HTMLDivElement>(null)
  const [images, setImages] = useState<ImageInfo[]>([])
  const [dragStart, setDragStart] = useState<PxPoint | null>(null)
  const [dragCur, setDragCur] = useState<PxPoint | null>(null)

  // 影像清單：mount 時／該頁版本變動（插入、取代後）重新抓取，確保 index 與最新狀態一致。
  useEffect(() => {
    let cancelled = false
    listPageImages(docId, page)
      .then((res) => {
        if (!cancelled) setImages(res)
      })
      .catch((err) => console.error('listPageImages failed:', err))
    return () => {
      cancelled = true
    }
  }, [docId, page, version])

  const localPoint = (e: { clientX: number; clientY: number }): PxPoint => {
    const r = overlayRef.current!.getBoundingClientRect()
    return { x: e.clientX - r.left, y: e.clientY - r.top }
  }

  const onPointerDown = (e: React.PointerEvent) => {
    if (!insertArmed) return
    overlayRef.current?.setPointerCapture(e.pointerId)
    const p = localPoint(e)
    setDragStart(p)
    setDragCur(p)
  }

  const onPointerMove = (e: React.PointerEvent) => {
    if (!insertArmed || !dragStart) return
    setDragCur(localPoint(e))
  }

  const onPointerUp = (e: React.PointerEvent) => {
    if (!insertArmed) return
    try {
      overlayRef.current?.releasePointerCapture(e.pointerId)
    } catch {
      /* pointer capture already released */
    }
    if (!dragStart || !dragCur) return
    const rectPx = pxRect(dragStart, dragCur)
    const clickPx = dragStart
    setDragStart(null)
    setDragCur(null)
    const rectPt: Rect = {
      x: rectPx.x / scale,
      y: rectPx.y / scale,
      w: rectPx.w / scale,
      h: rectPx.h / scale,
    }
    if (rectPt.w < CLICK_PT && rectPt.h < CLICK_PT) {
      if (!insertNaturalPt) return // 讀不到原始尺寸（理論上不會發生），忽略誤觸
      let w = insertNaturalPt.w
      let h = insertNaturalPt.h
      const shrink = Math.min(1, pageWidth / w, pageHeight / h)
      w *= shrink
      h *= shrink
      const x = Math.min(Math.max(clickPx.x / scale, 0), Math.max(0, pageWidth - w))
      const y = Math.min(Math.max(clickPx.y / scale, 0), Math.max(0, pageHeight - h))
      onInsertRectChange({ x, y, w, h })
      return
    }
    onInsertRectChange(rectPt)
  }

  const previewPx = dragStart && dragCur ? pxRect(dragStart, dragCur) : null

  return (
    <div
      ref={overlayRef}
      className={`image-layer ${insertArmed ? 'image-layer-insert-active' : ''}`}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
    >
      {!insertArmed &&
        images.map((img) => (
          <div
            key={img.index}
            className={`image-obj-box ${img.index === selectedIndex ? 'selected' : ''}`}
            style={{ left: img.x * scale, top: img.y * scale, width: img.w * scale, height: img.h * scale }}
            title={`${img.pxWidth} × ${img.pxHeight}px`}
            onClick={() => onSelectImage(img)}
          >
            <span className="image-obj-badge">
              {img.pxWidth}×{img.pxHeight}
            </span>
          </div>
        ))}

      {previewPx && (
        <div
          className="crop-select-rect"
          style={{ left: previewPx.x, top: previewPx.y, width: previewPx.w, height: previewPx.h }}
        />
      )}
    </div>
  )
}
