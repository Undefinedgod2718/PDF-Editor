import { useCallback, useEffect, useRef, useState } from 'react'
import {
  clearRecent,
  listRecent,
  recentThumbUrl,
  RecentEntryGoneError,
  removeRecent,
  setRecentStarred,
  type RecentEntry,
} from '../api'

export interface RecentPanelProps {
  /** 父層負責實際開檔（加分頁）；本元件只負責挑出路徑。 */
  onOpenPath: (path: string) => void | Promise<void>
  /** 父層呼叫系統開檔對話框（openDialog）；本元件不知道也不需要知道細節。 */
  onBrowse: () => void | Promise<void>
}

type SidebarTab = 'recent' | 'starred' | 'computer'

// ---------- 日期／大小格式化 ----------

const timeFmt = new Intl.DateTimeFormat('zh-TW', { hour: 'numeric', minute: '2-digit', hour12: true })
const monthDayFmt = new Intl.DateTimeFormat('zh-TW', { month: 'long', day: 'numeric' })
const fullDateFmt = new Intl.DateTimeFormat('zh-TW', { year: 'numeric', month: 'long', day: 'numeric' })

function isSameDay(a: Date, b: Date): boolean {
  return a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth() && a.getDate() === b.getDate()
}

/** Acrobat 風格的相對時間：今天顯示時間、昨天顯示「昨天」、
 *  其餘顯示月日（跨年再帶年份）。格式化一律交給 Intl.DateTimeFormat。 */
function formatOpenedAt(iso: string): string {
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return '—'

  const now = new Date()
  if (isSameDay(d, now)) {
    // hour12 的 zh-TW 輸出是「上午10:35」沒有空格，這裡用 formatToParts
    // 自己組出規格要求的「上午 10:35」，格式化本身仍由 Intl 負責。
    const parts = timeFmt.formatToParts(d)
    const dayPeriod = parts.find((p) => p.type === 'dayPeriod')?.value ?? ''
    const rest = parts
      .filter((p) => p.type !== 'dayPeriod' && p.type !== 'literal')
      .map((p) => p.value)
      .join(':')
    return `今天，${dayPeriod} ${rest}`
  }

  const yesterday = new Date(now)
  yesterday.setDate(now.getDate() - 1)
  if (isSameDay(d, yesterday)) return '昨天'

  if (d.getFullYear() === now.getFullYear()) return monthDayFmt.format(d)
  return fullDateFmt.format(d)
}

/** 檔案大小人性化顯示；size 為 null（檔案已消失）時交給呼叫端處理。 */
function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  const units = ['KB', 'MB', 'GB', 'TB']
  let value = bytes / 1024
  let unitIndex = 0
  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024
    unitIndex += 1
  }
  const decimals = value < 10 ? 1 : 0
  return `${value.toFixed(decimals)} ${units[unitIndex]}`
}

/** 沒有縮圖時的通用 PDF 佔位圖示（inline SVG；CSP 不允許外部圖示資源）。 */
function PdfPlaceholderIcon() {
  return (
    <svg className="recent-thumb-placeholder" viewBox="0 0 32 40" aria-hidden="true">
      <path
        d="M4 1h15l9 9v28.5a1.5 1.5 0 0 1-1.5 1.5h-22.5a1.5 1.5 0 0 1-1.5-1.5v-36a1.5 1.5 0 0 1 1.5-1.5z"
        fill="var(--bg-toolbar)"
        stroke="var(--border)"
      />
      <path d="M19 1v7.5a1.5 1.5 0 0 0 1.5 1.5h7.5" fill="none" stroke="var(--border)" />
      <text x="16" y="27" textAnchor="middle" fontSize="9" fontWeight="700" fill="var(--text-dim)">
        PDF
      </text>
    </svg>
  )
}

export default function RecentPanel({ onOpenPath, onBrowse }: RecentPanelProps) {
  const [entries, setEntries] = useState<RecentEntry[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [tab, setTab] = useState<SidebarTab>('recent')
  // 正在處理中的 path 集合：擋掉快速連點造成的重複 star/remove 呼叫。用 ref
  // 而非單純 state 是因為要在同一個 event handler 內同步擋下第二次點擊，
  // 不能等 React 重新 render 出新的 callback 才生效。version 只是用來
  // 觸發重繪，好讓 disabled/樣式反映 ref 目前的內容。
  const pendingRef = useRef<Set<string>>(new Set())
  const [, bumpPendingVersion] = useState(0)
  const [clearing, setClearing] = useState(false)

  const refresh = useCallback(async () => {
    try {
      const res = await listRecent()
      setEntries(res)
      setError(null)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh])

  // 每次異動都是「呼叫 API 再整份重抓」，不手動維護本地樂觀狀態，避免跟
  // 後端的星號分組／存在性判斷（exists）漂移。404（項目已不在清單）代表
  // 清單本來就過期了，資訊本身不是使用者需要看到的錯誤，靜靜重抓即可。
  const mutate = useCallback(
    async (path: string, fn: () => Promise<void>) => {
      if (pendingRef.current.has(path)) return
      pendingRef.current.add(path)
      bumpPendingVersion((n) => n + 1)
      try {
        await fn()
        setError(null)
      } catch (err) {
        // RecentEntryGoneError（404）＝清單過期而已，下面的 refresh 會把畫面
        // 對齊，不必打擾使用者。其他錯誤是真的失敗（伺服器沒回應之類），
        // 一律吞掉的話使用者會看到星號按了沒反應卻毫無線索。
        if (!(err instanceof RecentEntryGoneError)) {
          setError(err instanceof Error ? err.message : String(err))
        }
      } finally {
        await refresh()
        pendingRef.current.delete(path)
        bumpPendingVersion((n) => n + 1)
      }
    },
    [refresh],
  )

  const handleToggleStar = (entry: RecentEntry) =>
    mutate(entry.path, () => setRecentStarred(entry.path, !entry.starred))

  const handleRemove = (entry: RecentEntry) => mutate(entry.path, () => removeRecent(entry.path))

  const handleRowClick = (entry: RecentEntry) => {
    if (pendingRef.current.has(entry.path)) return
    if (!entry.exists) {
      const ok = window.confirm(`找不到「${entry.filename}」，是否從最近清單中移除這個項目？`)
      if (ok) void handleRemove(entry)
      return
    }
    void onOpenPath(entry.path)
  }

  const handleClear = async () => {
    if (clearing) return
    const ok = window.confirm('清除最近使用的檔案？已加上星號的項目不會被清除。')
    if (!ok) return
    setClearing(true)
    try {
      await clearRecent()
      await refresh()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setClearing(false)
    }
  }

  const renderRow = (entry: RecentEntry) => {
    const isPending = pendingRef.current.has(entry.path)
    return (
      <div
        key={entry.path}
        className={`recent-row${entry.exists ? '' : ' recent-row-missing'}`}
        onClick={() => handleRowClick(entry)}
      >
        <span className="recent-cell recent-cell-thumb">
          {entry.thumb_key ? (
            <img src={recentThumbUrl(entry.thumb_key)} alt="" draggable={false} />
          ) : (
            <PdfPlaceholderIcon />
          )}
        </span>
        <span className="recent-cell recent-cell-name" title={entry.path}>
          {entry.filename}
        </span>
        <span className="recent-cell recent-cell-date">{formatOpenedAt(entry.last_opened)}</span>
        <span className="recent-cell recent-cell-size">
          {entry.size === null ? '—' : formatSize(entry.size)}
        </span>
        <span className="recent-cell recent-cell-actions">
          <button
            className={`tb-btn recent-star-btn${entry.starred ? ' active' : ''}`}
            title={entry.starred ? '取消星號' : '加上星號'}
            disabled={isPending}
            onClick={(e) => {
              e.stopPropagation()
              void handleToggleStar(entry)
            }}
          >
            {entry.starred ? '★' : '☆'}
          </button>
          <button
            className="tb-btn recent-remove-btn"
            title="從清單中移除"
            disabled={isPending}
            onClick={(e) => {
              e.stopPropagation()
              void handleRemove(entry)
            }}
          >
            🗑
          </button>
        </span>
      </div>
    )
  }

  const renderRecentTab = () => {
    if (loading) return <div className="recent-loading">載入中…</div>

    if (entries.length === 0) {
      // 首次使用（清單全空）：沿用現有的歡迎卡片，而不是空白表格。
      return (
        <div className="welcome recent-welcome">
          <div className="welcome-card">
            <h1>PDF Editor</h1>
            <p>開啟 PDF 檔案開始檢視與編輯</p>
            <button className="btn btn-primary" onClick={() => void onBrowse()}>
              開啟 PDF
            </button>
          </div>
        </div>
      )
    }

    return (
      <>
        <h2 className="recent-heading">最近</h2>
        {error && <p className="error recent-error">{error}</p>}
        <div className="recent-table">{entries.map(renderRow)}</div>
        <div className="recent-footer">
          <button className="tb-btn recent-clear-btn" disabled={clearing} onClick={() => void handleClear()}>
            清除最近使用的檔案
          </button>
        </div>
      </>
    )
  }

  const renderStarredTab = () => {
    const starred = entries.filter((e) => e.starred)
    return (
      <>
        <h2 className="recent-heading">已加上星號</h2>
        {error && <p className="error recent-error">{error}</p>}
        {!loading && starred.length === 0 ? (
          <p className="recent-empty-hint">尚無加上星號的檔案</p>
        ) : (
          <div className="recent-table">{starred.map(renderRow)}</div>
        )}
      </>
    )
  }

  const renderComputerTab = () => (
    <div className="recent-computer-panel">
      <button className="btn btn-primary recent-browse-btn" onClick={() => void onBrowse()}>
        瀏覽…
      </button>
    </div>
  )

  return (
    <div className="recent-panel">
      <nav className="recent-sidebar">
        <button
          className={`recent-sidebar-item${tab === 'recent' ? ' active' : ''}`}
          onClick={() => setTab('recent')}
        >
          最近
        </button>
        <button
          className={`recent-sidebar-item${tab === 'starred' ? ' active' : ''}`}
          onClick={() => setTab('starred')}
        >
          已加上星號
        </button>
        <button
          className={`recent-sidebar-item${tab === 'computer' ? ' active' : ''}`}
          onClick={() => setTab('computer')}
        >
          您的電腦
        </button>
      </nav>
      <div className="recent-main">
        {tab === 'recent' && renderRecentTab()}
        {tab === 'starred' && renderStarredTab()}
        {tab === 'computer' && renderComputerTab()}
      </div>
    </div>
  )
}
