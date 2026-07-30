import type { AppMode } from '../api'

export interface TabBarTab {
  key: string
  title: string
  dirty: boolean
}

interface TabBarProps {
  tabs: TabBarTab[]
  activeKey: string | null
  onSelect: (key: string) => void
  onClose: (key: string) => void
  mode: AppMode
  busy: boolean
  onOpenLocalNewTab: () => void
  onOpenFileNewTab: (file: File) => void
  /** 開啟「最近使用的檔案」彈窗；只在 mode === 'local' 才會渲染對應按鈕。
   *  彈窗本身（含 RecentPanel）的狀態與 JSX 都留在 App.tsx，這裡只轉發一個回呼，
   *  跟 onOpenLocalNewTab／onOpenFileNewTab 是同一種「TabBar 不持有邏輯」的作法。 */
  onOpenRecentPanel: () => void
}

export default function TabBar({
  tabs,
  activeKey,
  onSelect,
  onClose,
  mode,
  busy,
  onOpenLocalNewTab,
  onOpenFileNewTab,
  onOpenRecentPanel,
}: TabBarProps) {
  return (
    <div className="tab-bar">
      {tabs.map((t) => (
        <div
          key={t.key}
          className={`tab${t.key === activeKey ? ' active' : ''}`}
          title={t.title}
          onClick={() => onSelect(t.key)}
        >
          {t.dirty && <span className="tab-dirty-dot" title="尚未存檔" />}
          <span className="tab-title">{t.title}</span>
          <button
            className="tab-close"
            title="關閉分頁"
            onClick={(e) => {
              e.stopPropagation()
              onClose(t.key)
            }}
          >
            ×
          </button>
        </div>
      ))}
      {mode === 'local' ? (
        <>
          <button className="tab-new" title="開啟新分頁" disabled={busy} onClick={onOpenLocalNewTab}>
            {busy ? '…' : '+'}
          </button>
          <button className="tab-recent" title="開啟最近使用的檔案" onClick={onOpenRecentPanel}>
            開啟…
          </button>
        </>
      ) : (
        <label className="tab-new" title="開啟新分頁">
          {busy ? '…' : '+'}
          <input
            type="file"
            accept="application/pdf"
            hidden
            disabled={busy}
            onChange={(e) => {
              const f = e.target.files?.[0]
              e.target.value = ''
              if (f) onOpenFileNewTab(f)
            }}
          />
        </label>
      )}
    </div>
  )
}
