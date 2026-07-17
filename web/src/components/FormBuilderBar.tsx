import type { NewFormField } from '../api'

export type BuilderFieldType = NewFormField['fieldType']

interface Props {
  selectedType: BuilderFieldType
  onSelectType: (type: BuilderFieldType) => void
  onDone: () => void
}

const TYPE_ORDER: BuilderFieldType[] = ['text', 'checkbox', 'radio', 'combobox', 'listbox', 'signature']

const TYPE_LABELS: Record<BuilderFieldType, string> = {
  text: '文字欄',
  checkbox: '核取方塊',
  radio: '單選群組',
  combobox: '下拉選單',
  listbox: '清單方塊',
  signature: '簽名欄位',
}

export default function FormBuilderBar({ selectedType, onSelectType, onDone }: Props) {
  return (
    <div className="form-builder-bar">
      <div className="crop-bar-header">
        <span>建立表單欄位</span>
        <button className="tb-btn" onClick={onDone}>
          ✕
        </button>
      </div>

      <div className="toolbar-group">
        {TYPE_ORDER.map((type) => (
          <button
            key={type}
            className={`fbb-btn ${selectedType === type ? 'active' : ''}`}
            onClick={() => onSelectType(type)}
          >
            {TYPE_LABELS[type]}
          </button>
        ))}
      </div>

      <div className="crop-bar-status">在頁面上拖曳繪製欄位；點選欄位可移動/縮放，雙擊編輯，Delete 刪除</div>

      <div className="crop-bar-actions">
        <button className="tb-btn btn-primary" onClick={onDone}>
          完成
        </button>
      </div>
    </div>
  )
}
