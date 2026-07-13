import { Suspense, lazy, useCallback, useEffect, useRef, useState } from 'react'
import { uploadStamp, type StampMeta } from '../api'
import type { ExcalidrawImperativeAPI, ExcalidrawInitialDataState } from '@excalidraw/excalidraw/types'

// Excalidraw bundle 較大，用動態 import 拆成獨立 chunk，只有進入繪圖模式才載入。
const ExcalidrawCanvas = lazy(() => import('./ExcalidrawCanvas'))

interface Props {
  docId: string
  page: number
  onDone: (stamp: StampMeta) => void
  onCancel: () => void
}

interface StoredScene {
  elements: ExcalidrawInitialDataState['elements']
  appState: {
    viewBackgroundColor?: string
    zoom?: { value: number }
    scrollX?: number
    scrollY?: number
  }
}

function sceneKey(docId: string, page: number): string {
  return `excal:${docId}:${page}`
}

/** 讀取該頁上次留下的 Excalidraw 草稿（若有），解析失敗一律視為沒有存檔。 */
function loadStoredScene(docId: string, page: number): ExcalidrawInitialDataState | null {
  try {
    const raw = localStorage.getItem(sceneKey(docId, page))
    if (!raw) return null
    const parsed = JSON.parse(raw) as StoredScene
    return {
      elements: parsed.elements,
      appState: parsed.appState as ExcalidrawInitialDataState['appState'],
    }
  } catch {
    return null
  }
}

/** 存檔／清除該頁草稿。localStorage 超限或序列化失敗只記錄，不阻斷關閉流程。 */
function persistScene(docId: string, page: number, api: ExcalidrawImperativeAPI) {
  try {
    const key = sceneKey(docId, page)
    const elements = api.getSceneElements()
    if (elements.length === 0) {
      localStorage.removeItem(key)
      return
    }
    const appState = api.getAppState()
    const stored: StoredScene = {
      elements,
      appState: {
        viewBackgroundColor: appState.viewBackgroundColor,
        zoom: { value: appState.zoom.value },
        scrollX: appState.scrollX,
        scrollY: appState.scrollY,
      },
    }
    localStorage.setItem(key, JSON.stringify(stored))
  } catch (err) {
    console.warn('儲存繪圖草稿失敗（已忽略）:', err)
  }
}

export default function DrawingModal({ docId, page, onDone, onCancel }: Props) {
  const apiRef = useRef<ExcalidrawImperativeAPI | null>(null)
  const [initialData] = useState<ExcalidrawInitialDataState | null>(() => loadStoredScene(docId, page))
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const handleCancel = useCallback(() => {
    if (apiRef.current) persistScene(docId, page, apiRef.current)
    onCancel()
  }, [docId, page, onCancel])

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
    const api = apiRef.current
    if (!api) return
    const elements = api.getSceneElements()
    if (elements.length === 0) {
      // 空畫布：直接關閉不上傳
      localStorage.removeItem(sceneKey(docId, page))
      onCancel()
      return
    }
    persistScene(docId, page, api)
    setBusy(true)
    setError(null)
    try {
      const { exportToBlob } = await import('@excalidraw/excalidraw')
      const appState = api.getAppState()
      const blob = await exportToBlob({
        elements,
        appState: { ...appState, exportBackground: false },
        files: api.getFiles(),
        mimeType: 'image/png',
        quality: 1,
      })
      const shortId = docId.slice(0, 8)
      const file = new File([blob], `drawing_${shortId}_p${page}.png`, { type: 'image/png' })
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
      className="modal-overlay drawing-modal-overlay"
      onPointerDown={(e) => {
        if (e.target === e.currentTarget) handleCancel()
      }}
    >
      <div className="modal drawing-modal">
        <div className="modal-header">
          <span>繪圖模式</span>
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
        <div className="drawing-modal-body">
          <Suspense fallback={<div className="annot-empty">載入繪圖工具中…</div>}>
            <ExcalidrawCanvas
              initialData={initialData}
              onApiReady={(api) => {
                apiRef.current = api
              }}
            />
          </Suspense>
        </div>
      </div>
    </div>
  )
}
