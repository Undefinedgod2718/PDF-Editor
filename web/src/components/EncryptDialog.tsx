import { useState } from 'react'
import { encryptDocument, type DocInfo, type PermissionFlags } from '../api'
import { DEFAULT_PERMISSIONS, PERMISSION_LABELS, PERMISSION_ORDER } from './ProtectDialog'

interface Props {
  doc: DocInfo
  onClose: () => void
}

/** 真正的開檔密碼加密（Phase 12）。與 ProtectDialog（Phase 11，權限限制）不同，
 *  這裡輸出的檔案沒有密碼就無法開啟／渲染——連本編輯器都不行——所以只提供
 *  「加密並下載」，不會存回文件庫，原文件維持不變。 */
export default function EncryptDialog({ doc, onClose }: Props) {
  const [userPassword, setUserPassword] = useState('')
  const [ownerPassword, setOwnerPassword] = useState('')
  const [permissions, setPermissions] = useState<PermissionFlags>(DEFAULT_PERMISSIONS)
  const [filename, setFilename] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const defaultFilename = `encrypted_${doc.filename}`

  const togglePermission = (key: keyof PermissionFlags) => {
    setPermissions((p) => {
      const next = { ...p, [key]: !p[key] }
      // High-quality print is meaningless / inconsistent without print.
      if (key === 'print' && !next.print) next.printHighQuality = false
      return next
    })
  }

  const submit = async () => {
    if (!userPassword) return
    setBusy(true)
    setError(null)
    try {
      await encryptDocument(doc.id, {
        userPassword,
        ownerPassword: ownerPassword.trim() || undefined,
        permissions,
        filename: filename.trim() || undefined,
      })
      onClose()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div
      className="modal-overlay"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose()
      }}
    >
      <div className="modal">
        <div className="modal-header">
          <span>加密文件</span>
          <button className="tb-btn" onClick={onClose}>
            ✕
          </button>
        </div>
        <div className="modal-body">
          {error && <div className="annot-hint">{error}</div>}
          <div className="annot-hint">
            加密會下載一份含開檔密碼的新檔案（另存為含密碼的副本），不會存回文件庫；原文件維持不變、仍可直接檢視。
            請妥善保管密碼——遺失後包含本編輯器在內都無法開啟。
          </div>

          <div className="modal-subtitle">開檔密碼（開啟文件時需要）</div>
          <input
            type="password"
            className="modal-input"
            value={userPassword}
            onChange={(e) => setUserPassword(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') void submit()
            }}
          />

          <div className="modal-subtitle">擁有者密碼（可選，預設同開檔密碼）</div>
          <input
            type="password"
            className="modal-input"
            value={ownerPassword}
            onChange={(e) => setOwnerPassword(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') void submit()
            }}
          />

          <div className="modal-subtitle">權限設定（取消勾選以限制該項操作）</div>
          {PERMISSION_ORDER.map((key) => (
            <label
              key={key}
              className="protect-permission-row"
              style={key === 'printHighQuality' ? { paddingLeft: '1.5em' } : undefined}
            >
              <input
                type="checkbox"
                checked={permissions[key]}
                disabled={key === 'printHighQuality' && !permissions.print}
                onChange={() => togglePermission(key)}
              />
              {PERMISSION_LABELS[key]}
            </label>
          ))}

          <div className="modal-subtitle">新檔名（可選）</div>
          <input
            className="modal-input"
            placeholder={defaultFilename}
            value={filename}
            onChange={(e) => setFilename(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') void submit()
            }}
          />

          <div className="modal-footer">
            <button
              className="tb-btn btn-primary"
              disabled={busy || !userPassword}
              onClick={() => void submit()}
            >
              {busy ? '加密中…' : '加密並下載'}
            </button>
            <button className="tb-btn" onClick={onClose}>
              取消
            </button>
          </div>
        </div>
      </div>
    </div>
  )
}
