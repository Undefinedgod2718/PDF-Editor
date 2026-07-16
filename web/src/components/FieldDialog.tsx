import { useState } from 'react'
import {
  createFormField,
  updateFormField,
  type FormField,
  type FormFieldUpdate,
  type NewFormField,
  type Rect,
} from '../api'

type BuilderFieldType = NewFormField['fieldType']

const TYPE_LABELS: Record<BuilderFieldType, string> = {
  text: '文字欄',
  checkbox: '核取方塊',
  radio: '單選群組',
  combobox: '下拉選單',
  listbox: '清單方塊',
  signature: '簽名欄位',
}

/** 欄位間垂直間距（points），radio 選項自動排列用。 */
const RADIO_GAP_PT = 8

type Props =
  | {
      mode: 'create'
      docId: string
      page: number
      fieldType: BuilderFieldType
      /** 頁面上拖曳出的矩形（view-space points，左上原點）。 */
      rectPt: Rect
      onClose: () => void
      onCreated: () => void | Promise<void>
    }
  | {
      mode: 'edit'
      docId: string
      page: number
      field: FormField
      onClose: () => void
      onUpdated: () => void | Promise<void>
    }

function parseOptionLines(raw: string): string[] {
  return raw
    .split('\n')
    .map((l) => l.trim())
    .filter((l) => l.length > 0)
}

/** 後端 fieldType 為 PDFium 命名（如 "ComboBox"），統一小寫比對前端型別鍵。 */
function normalizedType(field: FormField): string {
  return field.fieldType.toLowerCase()
}

export default function FieldDialog(props: Props) {
  const isEdit = props.mode === 'edit'
  const editType = isEdit ? normalizedType(props.field) : null
  const createType = !isEdit ? props.fieldType : null

  const isSignature = isEdit ? editType === 'signature' : createType === 'signature'
  const isChoice = isEdit
    ? editType === 'combobox' || editType === 'listbox'
    : createType === 'combobox' || createType === 'listbox'
  const isRadio = !isEdit && createType === 'radio'
  const isText = !isEdit && createType === 'text'

  const [name, setName] = useState(isEdit ? props.field.name : '')
  const [required, setRequired] = useState(false)
  const [requiredDirty, setRequiredDirty] = useState(false)
  const [multiline, setMultiline] = useState(false)
  const [fontSize, setFontSize] = useState(12)
  const [optionsText, setOptionsText] = useState(
    isEdit ? (props.field.options ?? []).join('\n') : '',
  )
  const [optionsDirty, setOptionsDirty] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const title = isEdit ? `編輯欄位 - ${props.field.name}` : `新增${TYPE_LABELS[props.fieldType]}`

  const submitCreate = async () => {
    if (props.mode !== 'create') return
    const trimmedName = name.trim()
    if (!trimmedName) {
      setError('名稱不可為空')
      return
    }

    let field: NewFormField
    if (props.fieldType === 'text') {
      field = {
        fieldType: 'text',
        name: trimmedName,
        rect: props.rectPt,
        multiline,
        required,
        fontSize,
      }
    } else if (props.fieldType === 'checkbox') {
      field = { fieldType: 'checkbox', name: trimmedName, rect: props.rectPt, required }
    } else if (props.fieldType === 'signature') {
      field = { fieldType: 'signature', name: trimmedName, rect: props.rectPt }
    } else if (props.fieldType === 'radio') {
      const opts = parseOptionLines(optionsText)
      if (opts.length < 2) {
        setError('單選群組至少需要 2 個選項')
        return
      }
      const rect = props.rectPt
      field = {
        fieldType: 'radio',
        name: trimmedName,
        required,
        options: opts.map((value, i) => ({
          value,
          rect: { x: rect.x, y: rect.y + i * (rect.h + RADIO_GAP_PT), w: rect.w, h: rect.h },
        })),
      }
    } else {
      // combobox / listbox
      const opts = parseOptionLines(optionsText)
      if (opts.length < 1) {
        setError('至少需要 1 個選項')
        return
      }
      field = {
        fieldType: props.fieldType,
        name: trimmedName,
        rect: props.rectPt,
        options: opts,
        required,
      }
    }

    setBusy(true)
    setError(null)
    try {
      await createFormField(props.docId, props.page, field)
      await props.onCreated()
      props.onClose()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  const submitEdit = async () => {
    if (props.mode !== 'edit') return
    const trimmedName = name.trim()
    if (!trimmedName) {
      setError('名稱不可為空')
      return
    }

    const update: FormFieldUpdate = {}
    if (trimmedName !== props.field.name) update.name = trimmedName
    if (requiredDirty) update.required = required
    if (optionsDirty && isChoice) {
      const opts = parseOptionLines(optionsText)
      if (opts.length < 1) {
        setError('至少需要 1 個選項')
        return
      }
      update.options = opts
    }

    if (Object.keys(update).length === 0) {
      props.onClose()
      return
    }

    setBusy(true)
    setError(null)
    try {
      await updateFormField(props.docId, props.page, props.field.index, update)
      await props.onUpdated()
      props.onClose()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  const submit = () => void (isEdit ? submitEdit() : submitCreate())

  return (
    <div
      className="modal-overlay"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) props.onClose()
      }}
    >
      <div className="modal">
        <div className="modal-header">
          <span>{title}</span>
          <button className="tb-btn" onClick={props.onClose}>
            ✕
          </button>
        </div>
        <div className="modal-body">
          {error && <div className="annot-hint">{error}</div>}

          <div className="modal-subtitle">名稱</div>
          <input
            className="modal-input"
            value={name}
            autoFocus
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') submit()
            }}
          />

          {!isSignature && (
            <label className="protect-permission-row">
              <input
                type="checkbox"
                checked={required}
                onChange={(e) => {
                  setRequired(e.target.checked)
                  setRequiredDirty(true)
                }}
              />
              必填
            </label>
          )}

          {isText && (
            <>
              <label className="protect-permission-row">
                <input
                  type="checkbox"
                  checked={multiline}
                  onChange={(e) => setMultiline(e.target.checked)}
                />
                多行
              </label>
              <div className="modal-subtitle">字級</div>
              <input
                type="number"
                className="modal-input"
                min={1}
                value={fontSize}
                onChange={(e) => setFontSize(Math.max(1, Number(e.target.value) || 12))}
              />
            </>
          )}

          {(isChoice || isRadio) && (
            <>
              <div className="modal-subtitle">選項（每行一個{isRadio ? '，至少 2 個' : '，至少 1 個'}）</div>
              <textarea
                className="modal-input"
                rows={5}
                value={optionsText}
                onChange={(e) => {
                  setOptionsText(e.target.value)
                  setOptionsDirty(true)
                }}
              />
            </>
          )}

          <div className="modal-footer">
            <button className="tb-btn btn-primary" disabled={busy} onClick={submit}>
              {busy ? '處理中…' : isEdit ? '儲存' : '建立'}
            </button>
            <button className="tb-btn" disabled={busy} onClick={props.onClose}>
              取消
            </button>
          </div>
        </div>
      </div>
    </div>
  )
}
