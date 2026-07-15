import { useRef, useState } from 'react'
import { insertImage, replaceImage, type DocInfo, type ImageInfo, type Rect } from '../api'

interface Props {
  doc: DocInfo
  currentPage: number
  /** 目前選取的既有影像（點擊外框選取），取代影像用。 */
  selectedImage: ImageInfo | null
  /** 是否已選好要插入的檔案，等待在頁面上拖曳/點擊放置。 */
  insertArmed: boolean
  /** 拖曳／點擊放置後的矩形（view-space points），尚未動作時為 null。 */
  insertRect: Rect | null
  /** 選好插入檔案後呼叫，帶出檔案原始尺寸換算成的 points（96dpi），供頁面點擊放置使用。 */
  onArmInsert: (naturalPt: { w: number; h: number }) => void
  /** 插入/取代成功後呼叫：重新整理該頁渲染版本（同其他頁面內容變更）。 */
  onApplied: () => void | Promise<void>
  /** 清空插入/選取的暫存互動狀態（取消插入、成功後皆會呼叫）。 */
  onReset: () => void
  /** 關閉影像模式（取消、Esc、成功後皆可觸發）。 */
  onClose: () => void
}

function readImageSize(file: File): Promise<{ width: number; height: number }> {
  return new Promise((resolve, reject) => {
    const url = URL.createObjectURL(file)
    const img = new Image()
    img.onload = () => {
      URL.revokeObjectURL(url)
      resolve({ width: img.naturalWidth, height: img.naturalHeight })
    }
    img.onerror = () => {
      URL.revokeObjectURL(url)
      reject(new Error('無法讀取影像檔案'))
    }
    img.src = url
  })
}

export default function ImageBar({
  doc,
  currentPage,
  selectedImage,
  insertArmed,
  insertRect,
  onArmInsert,
  onApplied,
  onReset,
  onClose,
}: Props) {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const insertFileRef = useRef<File | null>(null)

  const pickInsertFile = async (file: File) => {
    setError(null)
    try {
      const { width, height } = await readImageSize(file)
      insertFileRef.current = file
      onArmInsert({ w: (width * 72) / 96, h: (height * 72) / 96 })
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }

  const confirmInsert = async () => {
    const file = insertFileRef.current
    if (!file || !insertRect) return
    setBusy(true)
    setError(null)
    try {
      await insertImage(doc.id, currentPage, file, insertRect)
      insertFileRef.current = null
      await onApplied()
      onReset()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  const cancelInsert = () => {
    insertFileRef.current = null
    setError(null)
    onReset()
  }

  const doReplace = async (file: File) => {
    if (!selectedImage) return
    setBusy(true)
    setError(null)
    try {
      await replaceImage(doc.id, currentPage, selectedImage.index, file)
      await onApplied()
      onReset()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="crop-bar">
      <div className="crop-bar-header">
        <span>影像</span>
        <button className="tb-btn" onClick={onClose}>
          ✕
        </button>
      </div>

      {error && <div className="annot-hint">{error}</div>}

      {insertArmed ? (
        <>
          <div className="crop-bar-status">
            {insertRect
              ? `已選取範圍：${insertRect.w.toFixed(0)} × ${insertRect.h.toFixed(0)} pt`
              : '請在頁面上拖曳選取插入範圍，或直接點擊以原始大小插入'}
          </div>
          <div className="crop-bar-actions">
            <button
              className="tb-btn btn-primary"
              disabled={busy || !insertRect}
              onClick={() => void confirmInsert()}
            >
              {busy ? '插入中…' : '確認插入'}
            </button>
            <button className="tb-btn" disabled={busy} onClick={cancelInsert}>
              取消插入
            </button>
          </div>
        </>
      ) : (
        <>
          <div className="crop-bar-status">
            {selectedImage
              ? `已選取影像：${selectedImage.pxWidth} × ${selectedImage.pxHeight} px`
              : '點擊頁面上的影像外框以選取，或插入新影像'}
          </div>
          <div className="crop-bar-actions">
            <label className="tb-btn">
              取代影像
              <input
                type="file"
                accept="image/*"
                hidden
                disabled={!selectedImage || busy}
                onChange={(e) => {
                  const f = e.target.files?.[0]
                  e.target.value = ''
                  if (f) void doReplace(f)
                }}
              />
            </label>
            <label className="tb-btn btn-primary">
              插入影像
              <input
                type="file"
                accept="image/*"
                hidden
                disabled={busy}
                onChange={(e) => {
                  const f = e.target.files?.[0]
                  e.target.value = ''
                  if (f) void pickInsertFile(f)
                }}
              />
            </label>
            <button className="tb-btn" disabled={busy} onClick={onClose}>
              取消
            </button>
          </div>
        </>
      )}
    </div>
  )
}
