import { useCallback, useEffect, useRef, useState } from 'react'
import {
  detectMode,
  fetchDocInfo,
  getProtectionStatus,
  openByPath,
  openDialog,
  requestClose,
  saveDoc,
  setLocalFullscreen,
  uploadPdf,
  type AppMode,
  type DocInfo,
  type DocMeta,
} from './api'
import DocumentWorkspace from './components/DocumentWorkspace'
import TabBar from './components/TabBar'
import DecryptPrompt from './components/DecryptPrompt'
import RecentPanel from './components/RecentPanel'

declare global {
  interface Window {
    __PDF_EDITOR_STARTUP_DOC__?: string
    __PDF_EDITOR_STARTUP_ERROR__?: string
  }
}

interface Tab {
  key: string
  doc: DocInfo
}

/** 全螢幕中唯一常駐可見的退出入口（右上角，滑鼠靜止時淡出）；點擊一律直接
 *  exitFullscreen，不經過 App.tsx 全域 Escape 的 dialog-guard——那個 guard 只是
 *  避免「自動」的 Escape 搶在使用者手動關彈窗前面，使用者主動點這顆按鈕就是明確意圖，
 *  沒有理由再被擋。 */
function FullscreenExitHint({ visible, onExit }: { visible: boolean; onExit: () => void }) {
  return (
    <button
      type="button"
      className={`fullscreen-exit-hint${visible ? ' visible' : ''}`}
      title="按 Esc 或 F11 離開全螢幕"
      onClick={onExit}
    >
      ⤢ 離開全螢幕
    </button>
  )
}

export default function App() {
  // 桌面 vs 多人版：ping /api/local/ping 判斷，見 api.ts detectMode。
  const [mode, setMode] = useState<AppMode>('web')
  useEffect(() => {
    void detectMode().then(setMode)
  }, [])
  // 元件靠 CSS attribute selector 分流 local-only 樣式，不必逐層傳 prop。
  useEffect(() => {
    document.body.dataset.appMode = mode
  }, [mode])

  // ---- 全螢幕（沉浸閱讀）----
  // 狀態放在 App.tsx（不是各分頁）：tab bar／每個分頁的 toolbar 隱藏是全 app 一致的行為，
  // 桌面版的 OS 視窗也只有一個。CSS 隱藏 tab-bar／toolbar 靠 body[data-fullscreen]（見
  // app.css），跟 data-appMode 是同一套「元件不必逐層收 prop」的作法。
  const [fullscreen, setFullscreen] = useState(false)
  useEffect(() => {
    document.body.dataset.fullscreen = fullscreen ? 'true' : 'false'
  }, [fullscreen])

  // web 版：瀏覽器原生 Fullscreen API 的狀態變化一律以 `document.fullscreenElement` 為準——
  // 使用者按瀏覽器自己接管的 Esc、或透過瀏覽器 UI 離開全螢幕時，這裡才不會跟畫面（CSS 早就
  // 靠 data-fullscreen 隱藏了 chrome）兜不起來，變成「以為還在全螢幕其實已經跳出」的飄移。
  useEffect(() => {
    if (mode !== 'web') return
    const onFsChange = () => setFullscreen(!!document.fullscreenElement)
    document.addEventListener('fullscreenchange', onFsChange)
    return () => document.removeEventListener('fullscreenchange', onFsChange)
  }, [mode])

  // 桌面版兩個方向（進/出）都由我們自己呼叫端點決定，不會有「使用者用其他管道跳出」這回事，
  // 所以不必像 web 版另外監聽誰的原生事件——只要 enter/exitFullscreen 是唯一入口就不會分岔。
  const enterFullscreen = useCallback(async () => {
    if (mode === 'local') {
      try {
        await setLocalFullscreen(true)
        setFullscreen(true)
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e))
      }
      return
    }
    try {
      await document.documentElement.requestFullscreen()
      // 成功的話交給上面的 fullscreenchange 監聽器把 state 設成 true（單一事實來源）。
    } catch (e) {
      // 瀏覽器最常見的拒絕原因是「非使用者手勢直接觸發」（例如由某個非同步流程呼叫）。
      // 不能就此不了了之留在「按了看起來沒反應」的半吊子狀態——退而求其次只套用 CSS
      // 層的 chrome 隱藏，使用者仍看得到效果，也仍能用 Esc/F11/浮動按鈕退出。
      console.warn('requestFullscreen 被拒絕，退回純 CSS 全螢幕：', e)
      setFullscreen(true)
    }
  }, [mode])

  const exitFullscreen = useCallback(async () => {
    if (mode === 'local') {
      try {
        await setLocalFullscreen(false)
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e))
      } finally {
        // 即使端點失敗也要先退出 CSS 層：兩害相權，讓工具列至少肉眼可見比卡在
        // 「畫面說全螢幕、視窗其實不是」但工具列仍藏著更安全（見檔頭 spec 的
        // state-drift 說明——工具列被藏起來又沒有回去的路是最壞情況）。
        setFullscreen(false)
      }
      return
    }
    if (document.fullscreenElement) {
      try {
        await document.exitFullscreen()
        // 成功的話 fullscreenchange 監聽器會把 state 設回 false。
      } catch (e) {
        console.warn('exitFullscreen 失敗，仍強制退出 CSS 全螢幕：', e)
        setFullscreen(false)
      }
    } else {
      // requestFullscreen 之前被拒絕、走的是純 CSS fallback：沒有原生全螢幕可退，直接關 state。
      setFullscreen(false)
    }
  }, [mode])

  const toggleFullscreen = useCallback(() => {
    if (fullscreen) void exitFullscreen()
    else void enterFullscreen()
  }, [fullscreen, exitFullscreen, enterFullscreen])

  // 浮動的「離開全螢幕」提示：全螢幕中滑鼠移動/懸停時淡入，靜止一陣子後淡出——
  // 這是 spec 要求的「一定要有肉眼可見的退出方式」，不受下面 Escape guard 影響
  // （guard 只擋自動的 Escape 離開，這顆按鈕的 onClick 一律直接呼叫 exitFullscreen）。
  const [fsHintVisible, setFsHintVisible] = useState(true)
  useEffect(() => {
    if (!fullscreen) return
    setFsHintVisible(true)
    let timer = window.setTimeout(() => setFsHintVisible(false), 2200)
    const onMove = () => {
      setFsHintVisible(true)
      window.clearTimeout(timer)
      timer = window.setTimeout(() => setFsHintVisible(false), 2200)
    }
    window.addEventListener('mousemove', onMove)
    return () => {
      window.clearTimeout(timer)
      window.removeEventListener('mousemove', onMove)
    }
  }, [fullscreen])

  // ---- 分頁（多開 PDF）----
  const [tabs, setTabs] = useState<Tab[]>([])
  const [activeKey, setActiveKey] = useState<string | null>(null)
  const [dirtyMap, setDirtyMap] = useState<Record<string, boolean>>({})
  const nextKeyRef = useRef(0)
  /** 見 openPathNewTab：擋清單列被雙擊而開出兩個同一份文件的分頁。 */
  const openingPathRef = useRef(false)

  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  // 開新分頁時 GET /info 失敗，且偵測到是開檔密碼加密的文件：記錄 id／檔名，
  // 顯示「輸入密碼解密下載」提示，取代原本的死錯誤訊息。
  const [lockedDoc, setLockedDoc] = useState<{ id: string; filename?: string } | null>(null)

  const tryHandleEncrypted = useCallback(async (id: string, filename?: string): Promise<boolean> => {
    try {
      const status = await getProtectionStatus(id)
      if (status.protected) {
        setLockedDoc({ id, filename })
        return true
      }
    } catch {
      // 連 /protection 都失敗：不是加密造成的已知情境，交給原本的錯誤訊息處理。
    }
    return false
  }, [])

  const addTabFromId = useCallback(async (id: string) => {
    const info = await fetchDocInfo(id)
    const key = `tab-${nextKeyRef.current++}`
    setTabs((ts) => [...ts, { key, doc: info }])
    setActiveKey(key)
  }, [])

  const openFileNewTab = useCallback(
    async (file: File) => {
      setBusy(true)
      setError(null)
      setLockedDoc(null)
      let uploadedId: string | undefined
      try {
        const { id } = await uploadPdf(file)
        uploadedId = id
        await addTabFromId(id)
      } catch (e) {
        if (uploadedId && (await tryHandleEncrypted(uploadedId, file.name))) return
        setError(e instanceof Error ? e.message : String(e))
      } finally {
        setBusy(false)
      }
    },
    [addTabFromId, tryHandleEncrypted],
  )

  const openLocalNewTab = useCallback(async () => {
    setBusy(true)
    setError(null)
    setLockedDoc(null)
    try {
      const meta = await openDialog()
      if (!meta) return // 使用者取消
      await addTabFromId(meta.id)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }, [addTabFromId])

  // 從 RecentPanel／檔案關聯以外的「已知路徑」開檔（目前唯一入口是 RecentPanel 點選項目）。
  // 形狀比照 openFileNewTab：openByPath 先拿到 meta（含 id／filename），addTabFromId
  // 才是真正可能因為加密而失敗的一步，所以用 meta 是否已取得來判斷要不要嘗試
  // tryHandleEncrypted——跟 openFileNewTab 的 uploadedId 是同一個道理。
  const openPathNewTab = useCallback(
    async (path: string) => {
      // ref 而非 busy state：清單的列是「點一下就開」，使用者對檔案列表的
      // 肌肉記憶是雙擊，兩次 click 會在同一輪事件裡跑完，那時 setBusy 還沒
      // 反映出來，只有同步的 ref 擋得住重複開同一份文件的第二個分頁。
      // （`+` 按鈕是靠 disabled={busy} 擋，RecentPanel 的列沒有這層保護。）
      if (openingPathRef.current) return
      openingPathRef.current = true
      setBusy(true)
      setError(null)
      setLockedDoc(null)
      let meta: DocMeta | undefined
      try {
        meta = await openByPath(path)
        await addTabFromId(meta.id)
      } catch (e) {
        if (meta && (await tryHandleEncrypted(meta.id, meta.filename))) return
        setError(e instanceof Error ? e.message : String(e))
      } finally {
        openingPathRef.current = false
        setBusy(false)
      }
    },
    [addTabFromId, tryHandleEncrypted],
  )

  const openDocByIdNewTab = useCallback(
    async (id: string) => {
      setBusy(true)
      setError(null)
      setLockedDoc(null)
      try {
        await addTabFromId(id)
      } catch (e) {
        if (await tryHandleEncrypted(id)) return
        setError(e instanceof Error ? e.message : String(e))
      } finally {
        setBusy(false)
      }
    },
    [addTabFromId, tryHandleEncrypted],
  )

  const closeTab = useCallback(
    (key: string) => {
      if (dirtyMap[key]) {
        const discard = window.confirm('此分頁尚未存檔，放棄變更並關閉？')
        if (!discard) return
      }
      // 關掉最後一個分頁會回到歡迎／最近使用畫面（那裡沒有 tab bar／toolbar 可隱藏），
      // 不能把使用者留在「全螢幕但畫面空空」的狀態。算在 setTabs 外面：state updater
      // 必須是純函式，React 嚴格模式會重跑它，副作用寫在裡面會被執行兩次。
      if (tabs.length === 1 && tabs[0].key === key && fullscreen) void exitFullscreen()
      setTabs((ts) => ts.filter((t) => t.key !== key))
      setDirtyMap((m) => {
        const { [key]: _removed, ...rest } = m
        return rest
      })
      setActiveKey((cur) => {
        if (cur !== key) return cur
        const remaining = tabs.filter((t) => t.key !== key)
        return remaining.length > 0 ? remaining[remaining.length - 1].key : null
      })
    },
    [dirtyMap, tabs, fullscreen, exitFullscreen],
  )

  // 桌面版：檔案關聯／CLI 啟動時 Rust 已 open_path，init script 注入 doc id。一律開新分頁。
  useEffect(() => {
    const id = window.__PDF_EDITOR_STARTUP_DOC__
    const startupError = window.__PDF_EDITOR_STARTUP_ERROR__
    delete window.__PDF_EDITOR_STARTUP_DOC__
    delete window.__PDF_EDITOR_STARTUP_ERROR__
    if (id) {
      void openDocByIdNewTab(id)
    } else if (startupError) {
      setError(startupError)
    }
  }, [openDocByIdNewTab])

  // 桌面版：拖放檔案進視窗。Tauri 在 OS 層攔截 drag-drop（真實路徑，繞開瀏覽器 File API 的
  // 限制），Rust 端已直接 open_path 完成，這裡只收 dispatch 回來的事件走 openDocByIdNewTab。
  useEffect(() => {
    const onDrop = (e: Event) => {
      const id = (e as CustomEvent<{ id: string }>).detail?.id
      if (id) void openDocByIdNewTab(id)
    }
    const onDropError = (e: Event) => {
      setError((e as CustomEvent<{ message: string }>).detail?.message ?? '開檔失敗')
    }
    window.addEventListener('pdf-editor:open-doc', onDrop)
    window.addEventListener('pdf-editor:open-error', onDropError)
    return () => {
      window.removeEventListener('pdf-editor:open-doc', onDrop)
      window.removeEventListener('pdf-editor:open-error', onDropError)
    }
  }, [openDocByIdNewTab])

  // 桌面版：使用者按 X 關窗。Rust 一律先攔截（見 main.rs CloseRequested），這裡決定
  // 要不要真的關：沒有任何分頁未存檔就直接放行；有的話問過使用者，存檔對象是「所有」
  // 未存檔分頁（非單一 doc）。衝突在這裡不比照分頁內存檔提供強制覆寫，任何一個失敗就
  // 中止關窗，讓使用者回各分頁自己處理。
  useEffect(() => {
    const tryClose = async () => {
      try {
        await requestClose()
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e))
      }
    }
    const onCloseRequested = () => {
      void (async () => {
        if (mode !== 'local') return
        const dirtyTabs = tabs.filter((t) => dirtyMap[t.key])
        if (dirtyTabs.length === 0) {
          await tryClose()
          return
        }
        const wantsSave = window.confirm(
          `有 ${dirtyTabs.length} 個分頁尚未存檔，是否存檔後關閉？`,
        )
        if (wantsSave) {
          try {
            for (const t of dirtyTabs) await saveDoc(t.doc.id)
          } catch (e) {
            setError(e instanceof Error ? e.message : String(e))
            return
          }
          await tryClose()
          return
        }
        const wantsDiscard = window.confirm('不存檔直接關閉視窗？未存修改將遺失。')
        if (wantsDiscard) await tryClose()
      })()
    }
    window.addEventListener('pdf-editor:close-requested', onCloseRequested)
    return () => window.removeEventListener('pdf-editor:close-requested', onCloseRequested)
  }, [mode, tabs, dirtyMap])

  // 分頁開啟中，桌面版才有的「開啟最近使用的檔案」彈窗（TabBar 上的第二顆按鈕）。
  // web 版沒有本機路徑概念，這個彈窗只在 mode === 'local' 時才會被打開。
  const [recentModalOpen, setRecentModalOpen] = useState(false)

  // Escape 關閉彈窗；比照 DrawingModal / SignaturePad 的寫法，stopPropagation
  // 避免跟其他全域 Escape 監聽衝突。
  useEffect(() => {
    if (!recentModalOpen) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopPropagation()
        setRecentModalOpen(false)
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [recentModalOpen])

  // F11 切換全螢幕。這裡只綁 F11，不綁 Escape：Escape 在這個 app 已經被工具模式與一堆
  // 彈窗用滿了，要在 App 層決定「這一下 Escape 該不該離開全螢幕」就得把所有那些狀態
  // 鏡射上來。改成由 DocumentWorkspace 自己既有的 Escape handler 在最後 fallback 呼叫
  // onToggleFullscreen——判斷所需的狀態本來就在它手上，優先權自然正確。
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'F11') {
        e.preventDefault()
        toggleFullscreen()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [toggleFullscreen])

  if (tabs.length === 0) {
    if (mode === 'local') {
      // RecentPanel 清單全空時自己會渲染歡迎卡片（含「開啟 PDF」大按鈕），
      // 這裡不需要再疊一層 welcome-card。
      return (
        <div className="app-shell">
          <RecentPanel onOpenPath={openPathNewTab} onBrowse={openLocalNewTab} />
          {error && (
            <p className="error shell-error">
              {error}
              <button className="shell-error-dismiss" onClick={() => setError(null)}>
                ×
              </button>
            </p>
          )}
          {lockedDoc && (
            <DecryptPrompt
              id={lockedDoc.id}
              filename={lockedDoc.filename}
              onClose={() => setLockedDoc(null)}
            />
          )}
          {fullscreen && <FullscreenExitHint visible={fsHintVisible} onExit={() => void exitFullscreen()} />}
        </div>
      )
    }

    // web 版：沒有本機路徑，/api/local/* 一律 404，RecentPanel 絕不能掛載於此。
    // 維持原本的歡迎卡片不變。
    return (
      <div className="welcome">
        <div className="welcome-card">
          <h1>PDF Editor</h1>
          <p>開啟 PDF 檔案開始檢視與編輯</p>
          <label className="btn btn-primary">
            {busy ? '載入中…' : '開啟 PDF'}
            <input
              type="file"
              accept="application/pdf"
              hidden
              disabled={busy}
              onChange={(e) => {
                const f = e.target.files?.[0]
                if (f) void openFileNewTab(f)
              }}
            />
          </label>
          {error && <p className="error">{error}</p>}
        </div>
        {lockedDoc && (
          <DecryptPrompt
            id={lockedDoc.id}
            filename={lockedDoc.filename}
            onClose={() => setLockedDoc(null)}
          />
        )}
        {fullscreen && <FullscreenExitHint visible={fsHintVisible} onExit={() => void exitFullscreen()} />}
      </div>
    )
  }

  return (
    <div className="app-shell">
      <TabBar
        tabs={tabs.map((t) => ({
          key: t.key,
          title: t.doc.filename || '未命名',
          dirty: !!dirtyMap[t.key],
        }))}
        activeKey={activeKey}
        onSelect={setActiveKey}
        onClose={closeTab}
        mode={mode}
        busy={busy}
        onOpenLocalNewTab={openLocalNewTab}
        onOpenFileNewTab={openFileNewTab}
        onOpenRecentPanel={() => setRecentModalOpen(true)}
      />
      {error && (
        <p className="error shell-error">
          {error}
          <button className="shell-error-dismiss" onClick={() => setError(null)}>
            ×
          </button>
        </p>
      )}
      {tabs.map((t) => (
        <DocumentWorkspace
          key={t.key}
          initialDoc={t.doc}
          mode={mode}
          active={t.key === activeKey}
          onDirtyChange={(dirty) => setDirtyMap((m) => ({ ...m, [t.key]: dirty }))}
          onDocChange={(doc) => setTabs((ts) => ts.map((x) => (x.key === t.key ? { ...x, doc } : x)))}
          onOpenFileNewTab={openFileNewTab}
          onOpenLocalNewTab={openLocalNewTab}
          fullscreen={fullscreen}
          onToggleFullscreen={toggleFullscreen}
        />
      ))}
      {lockedDoc && (
        <DecryptPrompt
          id={lockedDoc.id}
          filename={lockedDoc.filename}
          onClose={() => setLockedDoc(null)}
        />
      )}
      {recentModalOpen && (
        <div
          className="modal-overlay"
          onMouseDown={(e) => {
            if (e.target === e.currentTarget) setRecentModalOpen(false)
          }}
        >
          <div className="modal modal-wide recent-modal">
            <div className="modal-header">
              <span>開啟最近使用的檔案</span>
              <button className="tb-btn" onClick={() => setRecentModalOpen(false)}>
                ✕
              </button>
            </div>
            <div className="modal-body">
              <RecentPanel
                onOpenPath={async (path) => {
                  setRecentModalOpen(false)
                  await openPathNewTab(path)
                }}
                onBrowse={async () => {
                  setRecentModalOpen(false)
                  await openLocalNewTab()
                }}
              />
            </div>
          </div>
        </div>
      )}
      {fullscreen && <FullscreenExitHint visible={fsHintVisible} onExit={() => void exitFullscreen()} />}
    </div>
  )
}
