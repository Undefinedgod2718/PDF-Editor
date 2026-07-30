/** Acrobat 風格頂部分類頁籤：把原本擠在兩排的 38 顆按鈕依用途分組。純呈現層——
 *  哪個控制項屬於哪個分類的實際內容分派在 Toolbar.tsx／AnnotToolbar.tsx 裡，
 *  這裡只負責畫頁籤列與記住使用者上次選的分類。 */
export type ToolTabId = 'all' | 'view' | 'annotate' | 'edit' | 'convert' | 'protect'

export const TOOL_TABS: { id: ToolTabId; label: string }[] = [
  { id: 'all', label: '所有工具' },
  { id: 'view', label: '檢視' },
  { id: 'annotate', label: '註解' },
  { id: 'edit', label: '編輯' },
  { id: 'convert', label: '轉換' },
  { id: 'protect', label: '保護' },
]

const VALID_IDS = new Set<string>(TOOL_TABS.map((t) => t.id))
const STORAGE_KEY = 'pdf-editor:toolTab'
const DEFAULT_TAB: ToolTabId = 'view'

/** 讀上次選的分類。localStorage 可能被清過、被別的版本寫進不認得的值、甚至手動竄改，
 *  一律當作沒存過，退回預設「檢視」，不丟例外。 */
export function loadStoredToolTab(): ToolTabId {
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    if (raw && VALID_IDS.has(raw)) return raw as ToolTabId
  } catch {
    // localStorage 不可用（無痕模式關閉／權限問題）：忽略，用預設值。
  }
  return DEFAULT_TAB
}

export function storeToolTab(id: ToolTabId): void {
  try {
    localStorage.setItem(STORAGE_KEY, id)
  } catch {
    // 存不進去（配額滿了等）不影響當次切換，只是下次不會記得。
  }
}

interface Props {
  active: ToolTabId
  onSelect: (id: ToolTabId) => void
}

export default function ToolTabs({ active, onSelect }: Props) {
  return (
    <div className="tool-tabs">
      {TOOL_TABS.map((t) => (
        <button
          key={t.id}
          type="button"
          className={`tool-tab ${active === t.id ? 'active' : ''}`}
          onClick={() => onSelect(t.id)}
        >
          {t.label}
        </button>
      ))}
    </div>
  )
}
