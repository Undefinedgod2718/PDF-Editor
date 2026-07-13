import type { Color } from '../api'

export type AnnotTool =
  | 'select'
  | 'highlight'
  | 'underline'
  | 'strikeout'
  | 'squiggly'
  | 'note'
  | 'ink'
  | 'freeText'
  | 'stamp'
  | 'editText'
  | 'draw'
  | 'form'
  | 'sign'

interface Props {
  tool: AnnotTool
  setTool: (t: AnnotTool) => void
  color: Color
  setColor: (c: Color) => void
  inkWidth: number
  setInkWidth: (w: number) => void
  showAnnotPanel: boolean
  toggleAnnotPanel: () => void
  /** 表單欄位已載入且該文件沒有任何欄位時為 true，顯示提示。 */
  noFormFields: boolean
}

const TOOLS: { id: AnnotTool; icon: string; title: string }[] = [
  { id: 'select', icon: '🖱️', title: '選取' },
  { id: 'highlight', icon: '🖍️', title: '螢光標記' },
  { id: 'underline', icon: 'U̲', title: '底線' },
  { id: 'strikeout', icon: 'S̶', title: '刪除線' },
  { id: 'squiggly', icon: '〜', title: '波浪線' },
  { id: 'note', icon: '📝', title: '便籤' },
  { id: 'ink', icon: '✏️', title: '手繪' },
  { id: 'freeText', icon: '🔤', title: '文字框' },
  { id: 'stamp', icon: '🖃', title: '印章' },
  { id: 'editText', icon: '✎', title: '編輯文字' },
  { id: 'draw', icon: '🎨', title: '繪圖' },
  { id: 'form', icon: '📋', title: '表單' },
  { id: 'sign', icon: '✍', title: '簽名' },
]

const PALETTE: Color[] = [
  { r: 255, g: 214, b: 0 }, // 黃
  { r: 255, g: 82, b: 82 }, // 紅
  { r: 76, g: 175, b: 80 }, // 綠
  { r: 76, g: 141, b: 255 }, // 藍
  { r: 255, g: 152, b: 0 }, // 橙
  { r: 171, g: 71, b: 188 }, // 紫
  { r: 236, g: 64, b: 122 }, // 粉
  { r: 0, g: 0, b: 0 }, // 黑
]

const INK_WIDTHS = [1, 2, 4, 8]

function colorToHex(c: Color): string {
  const h = (n: number) => n.toString(16).padStart(2, '0')
  return `#${h(c.r)}${h(c.g)}${h(c.b)}`
}

function hexToColor(hex: string): Color {
  const r = parseInt(hex.slice(1, 3), 16)
  const g = parseInt(hex.slice(3, 5), 16)
  const b = parseInt(hex.slice(5, 7), 16)
  return { r, g, b }
}

function sameColor(a: Color, b: Color): boolean {
  return a.r === b.r && a.g === b.g && a.b === b.b
}

export default function AnnotToolbar({
  tool,
  setTool,
  color,
  setColor,
  inkWidth,
  setInkWidth,
  showAnnotPanel,
  toggleAnnotPanel,
  noFormFields,
}: Props) {
  return (
    <div className="annot-toolbar">
      <div className="toolbar-group">
        {TOOLS.map((t) => (
          <button
            key={t.id}
            className={`tb-btn ${tool === t.id ? 'active' : ''}`}
            title={t.title}
            onClick={() => setTool(t.id)}
          >
            {t.icon}
          </button>
        ))}
      </div>

      <div className="toolbar-group annot-palette">
        {PALETTE.map((c) => (
          <button
            key={colorToHex(c)}
            className={`swatch ${sameColor(c, color) ? 'active' : ''}`}
            style={{ background: `rgb(${c.r},${c.g},${c.b})` }}
            title={colorToHex(c)}
            onClick={() => setColor(c)}
          />
        ))}
        <input
          type="color"
          className="swatch-picker"
          title="自訂顏色"
          value={colorToHex(color)}
          onChange={(e) => setColor(hexToColor(e.target.value))}
        />
      </div>

      {tool === 'ink' && (
        <div className="toolbar-group">
          {INK_WIDTHS.map((w) => (
            <button
              key={w}
              className={`tb-btn ${inkWidth === w ? 'active' : ''}`}
              title={`筆寬 ${w}`}
              onClick={() => setInkWidth(w)}
            >
              {w}
            </button>
          ))}
        </div>
      )}

      {tool === 'stamp' && <div className="annot-hint">請先在印章庫選擇印章，再於頁面拖曳出印章範圍</div>}

      {tool === 'form' && noFormFields && <div className="annot-hint">此文件無表單欄位</div>}

      <div className="toolbar-group" style={{ marginLeft: 'auto' }}>
        <button
          className={`tb-btn ${showAnnotPanel ? 'active' : ''}`}
          title="註解列表"
          onClick={toggleAnnotPanel}
        >
          📋
        </button>
      </div>
    </div>
  )
}
