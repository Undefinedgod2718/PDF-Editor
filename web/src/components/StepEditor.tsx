import { useEffect, useState } from 'react'
import {
  fetchOcrLanguages,
  type CompressPreset,
  type ExportFormat,
  type OcrLanguage,
  type PermissionFlags,
  type Step,
} from '../api'
import { parsePageSpec } from '../lib/pageSpec'
import { DEFAULT_PERMISSIONS, PERMISSION_LABELS, PERMISSION_ORDER } from './ProtectDialog'

// Wizard 建 pipeline 時沒有特定文件可對照頁數，所以頁碼規格只驗證格式（不
// 驗上限）——真正超出範圍要等 run 時對到實際文件才會知道，那時後端會報錯。
const NO_UPPER_BOUND = Number.MAX_SAFE_INTEGER

export const STEP_TYPE_ORDER: Step['type'][] = [
  'rotateAll',
  'crop',
  'resize',
  'compress',
  'protect',
  'encrypt',
  'ocr',
  'redact',
  'export',
]

export const STEP_TYPE_LABELS: Record<Step['type'], string> = {
  rotateAll: '旋轉全部頁面',
  crop: '裁切頁面',
  resize: '調整頁面大小',
  compress: '壓縮',
  protect: '保護（權限鎖）',
  encrypt: '加密（開檔密碼）',
  ocr: 'OCR 文字辨識',
  redact: '區域密文',
  export: '匯出',
}

export function defaultStepForType(type: Step['type']): Step {
  switch (type) {
    case 'rotateAll':
      return { type, delta: 90 }
    case 'crop':
      return { type, pages: [], rect: undefined }
    case 'resize':
      return { type, pages: [], width: 612, height: 792, mode: 'scale' }
    case 'compress':
      return { type, preset: 'ebook', dpi: undefined, quality: undefined }
    case 'protect':
      return { type, permissions: DEFAULT_PERMISSIONS }
    case 'encrypt':
      return { type, permissions: undefined }
    case 'ocr':
      return { type, langs: 'eng+chi_tra', dpi: undefined, min_confidence: undefined, force: false }
    case 'redact':
      return { type, boxes: [], dpi: undefined, jpeg_quality: undefined }
    case 'export':
      return { type, format: 'png', dpi: undefined, quality: undefined }
  }
}

/** 頁碼規格輸入框：本地維護原始文字，只有解析成功才回報 0-based 陣列給上層。 */
function PagesSpecField({
  pages,
  onChange,
  disabled,
}: {
  pages: number[]
  onChange: (pages: number[]) => void
  disabled?: boolean
}) {
  const toSpec = (ps: number[]) => ps.map((p) => p + 1).join(',')
  const [text, setText] = useState(toSpec(pages))
  const [err, setErr] = useState<string | null>(null)

  return (
    <>
      <div className="modal-subtitle">頁碼（1-based，如 1,3,5-9）</div>
      <input
        className="modal-input"
        placeholder="例：1,3,5-9"
        value={text}
        disabled={disabled}
        onChange={(e) => {
          const next = e.target.value
          setText(next)
          const trimmed = next.trim()
          if (trimmed === '') {
            setErr(null)
            onChange([])
            return
          }
          try {
            onChange(parsePageSpec(trimmed, NO_UPPER_BOUND))
            setErr(null)
          } catch (parseErr) {
            setErr(parseErr instanceof Error ? parseErr.message : String(parseErr))
          }
        }}
      />
      {err && <div className="annot-hint">{err}</div>}
    </>
  )
}

function PermissionCheckboxes({
  permissions,
  onChange,
  disabled,
}: {
  permissions: PermissionFlags
  onChange: (p: PermissionFlags) => void
  disabled?: boolean
}) {
  const toggle = (key: keyof PermissionFlags) => {
    const next = { ...permissions, [key]: !permissions[key] }
    if (key === 'print' && !next.print) next.printHighQuality = false
    onChange(next)
  }
  return (
    <>
      {PERMISSION_ORDER.map((key) => (
        <label
          key={key}
          className="protect-permission-row"
          style={key === 'printHighQuality' ? { paddingLeft: '1.5em' } : undefined}
        >
          <input
            type="checkbox"
            checked={permissions[key]}
            disabled={disabled || (key === 'printHighQuality' && !permissions.print)}
            onChange={() => toggle(key)}
          />
          {PERMISSION_LABELS[key]}
        </label>
      ))}
    </>
  )
}

const EXPORT_FORMAT_OPTIONS: { value: ExportFormat; label: string }[] = [
  { value: 'png', label: 'PNG' },
  { value: 'jpg', label: 'JPG' },
  { value: 'tiff', label: 'TIFF' },
  { value: 'pptx', label: 'PPTX' },
  { value: 'docx', label: 'Word (.docx)' },
  { value: 'xlsx', label: 'Excel (.xlsx)' },
  { value: 'markdown', label: 'Markdown (.md)' },
]
const EXPORT_RASTER_FORMATS: ExportFormat[] = ['png', 'jpg', 'tiff', 'pptx']

interface Props {
  step: Step
  onChange: (step: Step) => void
  disabled?: boolean
}

/** 單一 step 的參數表單。9 種 type 各自的欄位形狀對齊同名單發 endpoint。 */
export default function StepEditor({ step, onChange, disabled }: Props) {
  switch (step.type) {
    case 'rotateAll':
      return (
        <>
          <div className="modal-subtitle">旋轉角度</div>
          <select
            className="modal-input"
            value={step.delta}
            disabled={disabled}
            onChange={(e) => onChange({ ...step, delta: Number(e.target.value) })}
          >
            <option value={90}>順時針 90°</option>
            <option value={-90}>逆時針 90°</option>
            <option value={180}>180°</option>
          </select>
        </>
      )

    case 'crop': {
      const hasRect = step.rect !== undefined
      return (
        <>
          <PagesSpecField
            key="crop"
            pages={step.pages}
            onChange={(pages) => onChange({ ...step, pages })}
            disabled={disabled}
          />
          <label className="doc-list-item">
            <input
              type="checkbox"
              checked={hasRect}
              disabled={disabled}
              onChange={(e) =>
                onChange({ ...step, rect: e.target.checked ? { x: 0, y: 0, w: 100, h: 100 } : undefined })
              }
            />
            設定裁切範圍（不勾＝重設為整頁）
          </label>
          {step.rect && (
            <div className="export-quality-row">
              {(['x', 'y', 'w', 'h'] as const).map((axis) => (
                <input
                  key={axis}
                  type="number"
                  className="modal-input inline-num"
                  aria-label={axis}
                  placeholder={axis}
                  value={step.rect![axis]}
                  disabled={disabled}
                  onChange={(e) => onChange({ ...step, rect: { ...step.rect!, [axis]: Number(e.target.value) || 0 } })}
                />
              ))}
            </div>
          )}
        </>
      )
    }

    case 'resize':
      return (
        <>
          <PagesSpecField
            key="resize"
            pages={step.pages}
            onChange={(pages) => onChange({ ...step, pages })}
            disabled={disabled}
          />
          <div className="modal-subtitle">寬 × 高（points，36–14400）</div>
          <div className="export-quality-row">
            <input
              type="number"
              className="modal-input inline-num"
              min={36}
              max={14400}
              value={step.width}
              disabled={disabled}
              onChange={(e) => onChange({ ...step, width: Number(e.target.value) || 36 })}
            />
            <input
              type="number"
              className="modal-input inline-num"
              min={36}
              max={14400}
              value={step.height}
              disabled={disabled}
              onChange={(e) => onChange({ ...step, height: Number(e.target.value) || 36 })}
            />
          </div>
          <div className="modal-subtitle">模式</div>
          <select
            className="modal-input"
            value={step.mode}
            disabled={disabled}
            onChange={(e) => onChange({ ...step, mode: e.target.value as 'scale' | 'canvas' })}
          >
            <option value="scale">縮放內容以符合新尺寸</option>
            <option value="canvas">只改頁面框，內容置中</option>
          </select>
        </>
      )

    case 'compress':
      return (
        <>
          <div className="modal-subtitle">壓縮設定</div>
          <select
            className="modal-input"
            value={step.preset}
            disabled={disabled}
            onChange={(e) => onChange({ ...step, preset: e.target.value as CompressPreset })}
          >
            <option value="screen">螢幕（72 DPI／品質 60）</option>
            <option value="ebook">電子書（150 DPI／品質 75）</option>
            <option value="printer">印刷（300 DPI／品質 85）</option>
            <option value="custom">自訂</option>
          </select>
          {step.preset === 'custom' && (
            <div className="export-quality-row">
              <input
                type="number"
                className="modal-input inline-num"
                min={36}
                max={600}
                placeholder="DPI"
                value={step.dpi ?? ''}
                disabled={disabled}
                onChange={(e) => onChange({ ...step, dpi: Number(e.target.value) || undefined })}
              />
              <input
                type="number"
                className="modal-input inline-num"
                min={10}
                max={100}
                placeholder="品質"
                value={step.quality ?? ''}
                disabled={disabled}
                onChange={(e) => onChange({ ...step, quality: Number(e.target.value) || undefined })}
              />
            </div>
          )}
        </>
      )

    case 'protect':
      return (
        <>
          <div className="annot-hint">擁有者密碼於執行批次時另外輸入，不會存進此 pipeline。</div>
          <PermissionCheckboxes
            permissions={step.permissions}
            onChange={(permissions) => onChange({ ...step, permissions })}
            disabled={disabled}
          />
        </>
      )

    case 'encrypt': {
      const hasCustomPermissions = step.permissions !== undefined
      return (
        <>
          <div className="annot-hint">開檔密碼於執行批次時另外輸入，不會存進此 pipeline。</div>
          <label className="doc-list-item">
            <input
              type="checkbox"
              checked={hasCustomPermissions}
              disabled={disabled}
              onChange={(e) => onChange({ ...step, permissions: e.target.checked ? DEFAULT_PERMISSIONS : undefined })}
            />
            自訂權限（不勾＝全部允許）
          </label>
          {step.permissions && (
            <PermissionCheckboxes
              permissions={step.permissions}
              onChange={(permissions) => onChange({ ...step, permissions })}
              disabled={disabled}
            />
          )}
        </>
      )
    }

    case 'ocr':
      return <OcrStepFields step={step} onChange={onChange} disabled={disabled} />

    case 'redact':
      return <RedactStepFields step={step} onChange={onChange} disabled={disabled} />

    case 'export':
      return (
        <>
          <div className="modal-subtitle">格式（此 step 必須是 pipeline 最後一步）</div>
          <select
            className="modal-input"
            value={step.format}
            disabled={disabled}
            onChange={(e) => onChange({ ...step, format: e.target.value as ExportFormat })}
          >
            {EXPORT_FORMAT_OPTIONS.map((f) => (
              <option key={f.value} value={f.value}>
                {f.label}
              </option>
            ))}
          </select>
          {EXPORT_RASTER_FORMATS.includes(step.format) && (
            <>
              <div className="modal-subtitle">解析度 (DPI)</div>
              <input
                type="number"
                className="modal-input inline-num"
                min={36}
                max={600}
                value={step.dpi ?? ''}
                placeholder="預設"
                disabled={disabled}
                onChange={(e) => onChange({ ...step, dpi: Number(e.target.value) || undefined })}
              />
            </>
          )}
          {step.format === 'jpg' && (
            <>
              <div className="modal-subtitle">畫質（10–100）</div>
              <input
                type="number"
                className="modal-input inline-num"
                min={10}
                max={100}
                value={step.quality ?? ''}
                placeholder="預設"
                disabled={disabled}
                onChange={(e) => onChange({ ...step, quality: Number(e.target.value) || undefined })}
              />
            </>
          )}
        </>
      )
  }
}

function OcrStepFields({
  step,
  onChange,
  disabled,
}: {
  step: Extract<Step, { type: 'ocr' }>
  onChange: (step: Step) => void
  disabled?: boolean
}) {
  const FALLBACK_LANGUAGES: OcrLanguage[] = [
    { code: 'eng', label: '英文' },
    { code: 'chi_tra', label: '繁體中文' },
  ]
  const [languages, setLanguages] = useState<OcrLanguage[]>(FALLBACK_LANGUAGES)
  useEffect(() => {
    let cancelled = false
    fetchOcrLanguages()
      .then((langs) => {
        if (!cancelled && langs.length > 0) setLanguages(langs)
      })
      .catch(() => {
        // stays on FALLBACK_LANGUAGES
      })
    return () => {
      cancelled = true
    }
  }, [])

  const selected = new Set((step.langs ?? '').split('+').filter(Boolean))
  const toggleLang = (code: string) => {
    const next = new Set(selected)
    if (next.has(code)) next.delete(code)
    else next.add(code)
    onChange({ ...step, langs: [...next].join('+') || undefined })
  }

  return (
    <>
      <div className="modal-subtitle">語言</div>
      {languages.map((l) => (
        <label key={l.code} className="doc-list-item">
          <input type="checkbox" checked={selected.has(l.code)} disabled={disabled} onChange={() => toggleLang(l.code)} />
          <span>{l.label}</span>
        </label>
      ))}
      <label className="doc-list-item">
        <input
          type="checkbox"
          checked={step.force}
          disabled={disabled}
          onChange={(e) => onChange({ ...step, force: e.target.checked })}
        />
        強制重新辨識（含已有文字層的頁面）
      </label>
      <div className="modal-subtitle">信心門檻（0–100，可留空用預設）</div>
      <input
        type="number"
        className="modal-input inline-num"
        min={0}
        max={100}
        value={step.min_confidence ?? ''}
        placeholder="預設 60"
        disabled={disabled}
        onChange={(e) => onChange({ ...step, min_confidence: Number(e.target.value) || undefined })}
      />
    </>
  )
}

function RedactStepFields({
  step,
  onChange,
  disabled,
}: {
  step: Extract<Step, { type: 'redact' }>
  onChange: (step: Step) => void
  disabled?: boolean
}) {
  const updateBox = (i: number, patch: Partial<(typeof step.boxes)[number]>) => {
    const boxes = step.boxes.map((b, idx) => (idx === i ? { ...b, ...patch } : b))
    onChange({ ...step, boxes })
  }
  const removeBox = (i: number) => onChange({ ...step, boxes: step.boxes.filter((_, idx) => idx !== i) })
  const addBox = () => onChange({ ...step, boxes: [...step.boxes, { page: 0, x: 0, y: 0, w: 100, h: 20 }] })

  return (
    <>
      <div className="annot-hint">密文框對每份文件套用相同座標——不同尺寸/版面的文件請另存一條 pipeline。</div>
      <div className="doc-order-list">
        {step.boxes.map((box, i) => (
          <div key={i} className="doc-order-item">
            <input
              type="number"
              className="modal-input inline-num"
              aria-label="頁碼"
              placeholder="頁"
              min={1}
              value={box.page + 1}
              disabled={disabled}
              onChange={(e) => updateBox(i, { page: Math.max(0, (Number(e.target.value) || 1) - 1) })}
            />
            {(['x', 'y', 'w', 'h'] as const).map((axis) => (
              <input
                key={axis}
                type="number"
                className="modal-input inline-num"
                aria-label={axis}
                placeholder={axis}
                value={box[axis]}
                disabled={disabled}
                onChange={(e) => updateBox(i, { [axis]: Number(e.target.value) || 0 })}
              />
            ))}
            <div className="doc-order-actions">
              <button className="tb-btn" disabled={disabled} onClick={() => removeBox(i)}>
                移除
              </button>
            </div>
          </div>
        ))}
      </div>
      <div className="modal-footer" style={{ justifyContent: 'flex-start' }}>
        <button className="tb-btn" disabled={disabled} onClick={addBox}>
          + 新增密文框
        </button>
      </div>
      <div className="modal-subtitle">JPEG 品質（10–100，可留空用預設 90）</div>
      <input
        type="number"
        className="modal-input inline-num"
        min={10}
        max={100}
        value={step.jpeg_quality ?? ''}
        placeholder="預設 90"
        disabled={disabled}
        onChange={(e) => onChange({ ...step, jpeg_quality: Number(e.target.value) || undefined })}
      />
    </>
  )
}
