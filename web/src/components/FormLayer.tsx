import { setFormFieldValue, type FormField } from '../api'

interface Props {
  docId: string
  scale: number
  /** 已預先過濾成只屬於這一頁的欄位（見 Viewer.tsx）。 */
  fields: FormField[]
  /** 某欄位寫入成功後呼叫，通知上層 bump 該頁版本＋重新抓表單欄位。 */
  onFieldChanged: (page: number) => void
}

/** ComboBox / ListBox 本版不可寫，即使後端回報 writable 也一律當唯讀處理。 */
function isReadOnly(field: FormField): boolean {
  return !field.writable || field.fieldType === 'ComboBox' || field.fieldType === 'ListBox'
}

export default function FormLayer({ docId, scale, fields, onFieldChanged }: Props) {
  const submitText = async (field: FormField, value: string) => {
    if (value === (field.value ?? '')) return
    try {
      await setFormFieldValue(docId, field.page, field.index, { value })
      onFieldChanged(field.page)
    } catch (err) {
      console.error('setFormFieldValue (text) failed:', err)
    }
  }

  const toggleCheckbox = async (field: FormField) => {
    try {
      await setFormFieldValue(docId, field.page, field.index, { checked: !field.checked })
      onFieldChanged(field.page)
    } catch (err) {
      console.error('setFormFieldValue (checkbox) failed:', err)
    }
  }

  const selectRadio = async (field: FormField) => {
    // 不做 checked early-return：後端曾誤報未填 radio 為 checked（已修），
    // 重複點擊只是冪等寫入，保留可點性比省一次請求重要。
    try {
      await setFormFieldValue(docId, field.page, field.index, { checked: true })
      onFieldChanged(field.page)
    } catch (err) {
      console.error('setFormFieldValue (radio) failed:', err)
    }
  }

  return (
    <div className="form-layer">
      {fields.map((field) => {
        if (!field.rect) return null // 無 bounds 的欄位無法定位，跳過
        const style = {
          left: field.rect.x * scale,
          top: field.rect.y * scale,
          width: field.rect.w * scale,
          height: field.rect.h * scale,
        }
        const key = `${field.page}-${field.index}`

        if (isReadOnly(field)) {
          return (
            <div
              key={key}
              className="form-field form-field-readonly"
              style={style}
              title={`「${field.name}」本版不可寫（下拉／清單為唯讀）`}
            >
              {field.value ?? ''}
            </div>
          )
        }

        if (field.fieldType === 'Text') {
          return (
            <input
              // value 由伺服器成功寫回後才變動，靠它變化強迫重新掛載＝取得最新 defaultValue，
              // 編輯過程中不會因為同一個值而重掛，不影響輸入中的游標。
              key={`${key}-${field.value ?? ''}`}
              className="form-field form-field-text"
              style={style}
              defaultValue={field.value ?? ''}
              title={field.name}
              onPointerDown={(e) => e.stopPropagation()}
              onClick={(e) => e.stopPropagation()}
              onKeyDown={(e) => {
                e.stopPropagation()
                if (e.key === 'Enter') {
                  e.preventDefault()
                  ;(e.target as HTMLInputElement).blur()
                }
              }}
              onBlur={(e) => void submitText(field, e.target.value)}
            />
          )
        }

        if (field.fieldType === 'Checkbox') {
          return (
            <div
              key={key}
              className={`form-field form-field-checkbox ${field.checked ? 'checked' : ''}`}
              style={style}
              title={field.name}
              onPointerDown={(e) => e.stopPropagation()}
              onClick={(e) => {
                e.stopPropagation()
                void toggleCheckbox(field)
              }}
            >
              {field.checked ? '✓' : ''}
            </div>
          )
        }

        if (field.fieldType === 'RadioButton') {
          return (
            <div
              key={key}
              className={`form-field form-field-radio ${field.checked ? 'checked' : ''}`}
              style={style}
              title={field.name}
              onPointerDown={(e) => e.stopPropagation()}
              onClick={(e) => {
                e.stopPropagation()
                void selectRadio(field)
              }}
            >
              {field.checked ? '●' : ''}
            </div>
          )
        }

        // 未知型別：一律當唯讀徽章顯示，不嘗試寫入。
        return (
          <div key={key} className="form-field form-field-readonly" style={style} title={field.name}>
            {field.fieldType}
          </div>
        )
      })}
    </div>
  )
}
