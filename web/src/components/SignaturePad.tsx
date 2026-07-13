import { useCallback, useEffect, useRef, useState } from 'react'
import { uploadStamp, type StampMeta } from '../api'

interface Props {
  onDone: (stamp: StampMeta) => void
  onCancel: () => void
}

interface PxPoint {
  x: number
  y: number
}

const CANVAS_W = 640
const CANVAS_H = 300

const SIGN_COLORS = ['#000000', '#1e3a8a', '#b91c1c', '#065f46']
const SIGN_WIDTHS = [2, 4, 6]

export default function SignaturePad({ onDone, onCancel }: Props) {
  const canvasRef = useRef<HTMLCanvasElement>(null)
  const ctxRef = useRef<CanvasRenderingContext2D | null>(null)
  const drawingRef = useRef(false)
  const lastPointRef = useRef<PxPoint | null>(null)
  const hasDrawnRef = useRef(false)

  const [color, setColor] = useState(SIGN_COLORS[0])
  const [width, setWidth] = useState(2)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [empty, setEmpty] = useState(true)

  // 建立畫布：devicePixelRatio 縮放確保線條不糊，背景保持透明。
  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas) return
    const dpr = Math.min(window.devicePixelRatio || 1, 2)
    canvas.width = CANVAS_W * dpr
    canvas.height = CANVAS_H * dpr
    canvas.style.width = `${CANVAS_W}px`
    canvas.style.height = `${CANVAS_H}px`
    const ctx = canvas.getContext('2d')
    if (ctx) {
      ctx.scale(dpr, dpr)
      ctx.lineCap = 'round'
      ctx.lineJoin = 'round'
    }
    ctxRef.current = ctx
  }, [])

  const localPoint = (e: { clientX: number; clientY: number }): PxPoint => {
    const r = canvasRef.current!.getBoundingClientRect()
    return { x: e.clientX - r.left, y: e.clientY - r.top }
  }

  const onPointerDown = (e: React.PointerEvent<HTMLCanvasElement>) => {
    e.stopPropagation()
    canvasRef.current?.setPointerCapture(e.pointerId)
    drawingRef.current = true
    lastPointRef.current = localPoint(e)
  }

  const onPointerMove = (e: React.PointerEvent<HTMLCanvasElement>) => {
    e.stopPropagation()
    if (!drawingRef.current) return
    const ctx = ctxRef.current
    const last = lastPointRef.current
    if (!ctx || !last) return
    const p = localPoint(e)
    ctx.strokeStyle = color
    ctx.lineWidth = width
    ctx.beginPath()
    ctx.moveTo(last.x, last.y)
    ctx.lineTo(p.x, p.y)
    ctx.stroke()
    lastPointRef.current = p
    if (!hasDrawnRef.current) {
      hasDrawnRef.current = true
      setEmpty(false)
    }
  }

  const onPointerUp = (e: React.PointerEvent<HTMLCanvasElement>) => {
    e.stopPropagation()
    try {
      canvasRef.current?.releasePointerCapture(e.pointerId)
    } catch {
      /* pointer capture already released */
    }
    drawingRef.current = false
    lastPointRef.current = null
  }

  const handleClear = () => {
    const canvas = canvasRef.current
    const ctx = ctxRef.current
    if (!canvas || !ctx) return
    ctx.clearRect(0, 0, CANVAS_W, CANVAS_H)
    hasDrawnRef.current = false
    setEmpty(true)
  }

  const handleCancel = useCallback(() => {
    onCancel()
  }, [onCancel])

  // Escape 關閉本 modal；stopPropagation 避免與 App 全域 Escape（切回 select 工具）衝突。
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopPropagation()
        handleCancel()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [handleCancel])

  const handleComplete = async () => {
    if (empty) {
      // 空畫布：直接關閉不上傳
      onCancel()
      return
    }
    const canvas = canvasRef.current
    if (!canvas) return
    setBusy(true)
    setError(null)
    try {
      const blob = await new Promise<Blob | null>((resolve) => canvas.toBlob(resolve, 'image/png'))
      if (!blob) throw new Error('產生簽名圖片失敗')
      const file = new File([blob], `signature_${Date.now()}.png`, { type: 'image/png' })
      const meta = await uploadStamp(file)
      onDone(meta)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div
      className="modal-overlay"
      onPointerDown={(e) => {
        if (e.target === e.currentTarget) handleCancel()
      }}
    >
      <div className="modal signature-modal">
        <div className="modal-header">
          <span>簽名板</span>
          <div className="toolbar-group" style={{ marginLeft: 'auto' }}>
            {error && <span className="error drawing-modal-error">{error}</span>}
            <button className="tb-btn" onClick={handleCancel} disabled={busy}>
              取消
            </button>
            <button className="tb-btn btn-primary" onClick={() => void handleComplete()} disabled={busy}>
              {busy ? '處理中…' : '完成並蓋章'}
            </button>
          </div>
        </div>
        <div className="signature-pad-body">
          <div className="signature-toolbar">
            <div className="toolbar-group annot-palette">
              {SIGN_COLORS.map((c) => (
                <button
                  key={c}
                  className={`swatch ${color === c ? 'active' : ''}`}
                  style={{ background: c }}
                  title={c}
                  onClick={() => setColor(c)}
                />
              ))}
            </div>
            <div className="toolbar-group">
              {SIGN_WIDTHS.map((w) => (
                <button
                  key={w}
                  className={`tb-btn ${width === w ? 'active' : ''}`}
                  title={`筆寬 ${w}`}
                  onClick={() => setWidth(w)}
                >
                  {w}
                </button>
              ))}
            </div>
            <button className="tb-btn" style={{ marginLeft: 'auto' }} onClick={handleClear}>
              清除
            </button>
          </div>
          <div className="signature-canvas-wrap">
            <canvas
              ref={canvasRef}
              onPointerDown={onPointerDown}
              onPointerMove={onPointerMove}
              onPointerUp={onPointerUp}
            />
          </div>
        </div>
      </div>
    </div>
  )
}
