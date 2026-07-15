import { useEffect, useState } from 'react'
import {
  getProtectionStatus,
  protectDocument,
  unprotectDocument,
  type DocInfo,
  type DocMeta,
  type PermissionFlags,
} from '../api'

interface Props {
  doc: DocInfo
  onClose: () => void
  onOpenDoc: (id: string) => void | Promise<void>
}

export const DEFAULT_PERMISSIONS: PermissionFlags = {
  print: true,
  printHighQuality: true,
  modify: true,
  copy: true,
  copyForAccessibility: true,
  annotate: true,
  fillForms: true,
  assemble: true,
}

export const PERMISSION_LABELS: Record<keyof PermissionFlags, string> = {
  print: '允許列印',
  printHighQuality: '允許高品質列印',
  modify: '允許修改內容',
  copy: '允許複製文字/圖片',
  copyForAccessibility: '允許輔助技術複製（無障礙）',
  annotate: '允許加註解/意見',
  fillForms: '允許填寫表單欄位',
  assemble: '允許組織頁面（插入/刪除/旋轉頁面）',
}

export const PERMISSION_ORDER: (keyof PermissionFlags)[] = [
  'print',
  'printHighQuality',
  'modify',
  'copy',
  'copyForAccessibility',
  'annotate',
  'fillForms',
  'assemble',
]

function ProtectResult({
  document,
  message,
  onClose,
  onOpenDoc,
}: {
  document: DocMeta
  message: string
  onClose: () => void
  onOpenDoc: (id: string) => void | Promise<void>
}) {
  return (
    <div className="modal-body">
      <p>{message}</p>
      <p>新文件已建立：{document.filename}</p>
      <div className="modal-footer">
        <button className="tb-btn btn-primary" onClick={() => void onOpenDoc(document.id)}>
          開啟新文件
        </button>
        <button className="tb-btn" onClick={onClose}>
          關閉
        </button>
      </div>
    </div>
  )
}

export default function ProtectDialog({ doc, onClose, onOpenDoc }: Props) {
  const [loadingStatus, setLoadingStatus] = useState(true)
  const [statusError, setStatusError] = useState<string | null>(null)
  const [isProtected, setIsProtected] = useState(false)
  const [currentPermissions, setCurrentPermissions] = useState<PermissionFlags | null>(null)

  const [permissions, setPermissions] = useState<PermissionFlags>(DEFAULT_PERMISSIONS)
  const [ownerPassword, setOwnerPassword] = useState('')
  const [unprotectPassword, setUnprotectPassword] = useState('')
  const [filename, setFilename] = useState('')

  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [result, setResult] = useState<{ document: DocMeta; message: string } | null>(null)

  useEffect(() => {
    let cancelled = false
    setLoadingStatus(true)
    setStatusError(null)
    getProtectionStatus(doc.id)
      .then((status) => {
        if (cancelled) return
        setIsProtected(status.protected)
        setCurrentPermissions(status.permissions)
      })
      .catch((err) => {
        if (cancelled) return
        setStatusError(err instanceof Error ? err.message : String(err))
      })
      .finally(() => {
        if (!cancelled) setLoadingStatus(false)
      })
    return () => {
      cancelled = true
    }
  }, [doc.id])

  const defaultProtectFilename = `protected_${doc.filename}`
  const defaultUnprotectFilename = `unprotected_${doc.filename}`

  const togglePermission = (key: keyof PermissionFlags) => {
    setPermissions((p) => {
      const next = { ...p, [key]: !p[key] }
      // High-quality print is meaningless / inconsistent without print.
      if (key === 'print' && !next.print) next.printHighQuality = false
      return next
    })
  }

  const submitProtect = async () => {
    setBusy(true)
    setError(null)
    try {
      const res = await protectDocument(doc.id, ownerPassword, permissions, filename.trim() || undefined)
      setResult({ document: res.document, message: '文件已加上保護。' })
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  const submitUnprotect = async () => {
    setBusy(true)
    setError(null)
    try {
      const res = await unprotectDocument(doc.id, unprotectPassword, filename.trim() || undefined)
      setResult({ document: res.document, message: '文件保護已移除。' })
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
          <span>保護文件</span>
          <button className="tb-btn" onClick={onClose}>
            ✕
          </button>
        </div>
        {result ? (
          <ProtectResult
            document={result.document}
            message={result.message}
            onClose={onClose}
            onOpenDoc={onOpenDoc}
          />
        ) : loadingStatus ? (
          <div className="modal-body">
            <p>正在讀取保護狀態…</p>
          </div>
        ) : (
          <div className="modal-body">
            {statusError && <div className="annot-hint">{statusError}</div>}
            {error && <div className="annot-hint">{error}</div>}

            {isProtected ? (
              <>
                <div className="modal-subtitle">此文件目前已受保護</div>
                {currentPermissions && (
                  <ul className="protect-summary">
                    {PERMISSION_ORDER.map((key) => (
                      <li key={key}>
                        {currentPermissions[key] ? '✓' : '✕'} {PERMISSION_LABELS[key]}
                      </li>
                    ))}
                  </ul>
                )}

                <div className="modal-subtitle">擁有者密碼</div>
                <input
                  type="password"
                  className="modal-input"
                  value={unprotectPassword}
                  onChange={(e) => setUnprotectPassword(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') void submitUnprotect()
                  }}
                />

                <div className="modal-subtitle">新檔名（可選）</div>
                <input
                  className="modal-input"
                  placeholder={defaultUnprotectFilename}
                  value={filename}
                  onChange={(e) => setFilename(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') void submitUnprotect()
                  }}
                />

                <div className="modal-footer">
                  <button
                    className="tb-btn btn-primary"
                    disabled={busy || !unprotectPassword}
                    onClick={() => void submitUnprotect()}
                  >
                    {busy ? '處理中…' : '移除保護'}
                  </button>
                  <button className="tb-btn" onClick={onClose}>
                    取消
                  </button>
                </div>
              </>
            ) : (
              <>
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

                <div className="modal-subtitle">擁有者密碼</div>
                <input
                  type="password"
                  className="modal-input"
                  value={ownerPassword}
                  onChange={(e) => setOwnerPassword(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') void submitProtect()
                  }}
                />

                <div className="modal-subtitle">新檔名（可選）</div>
                <input
                  className="modal-input"
                  placeholder={defaultProtectFilename}
                  value={filename}
                  onChange={(e) => setFilename(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') void submitProtect()
                  }}
                />

                <div className="modal-footer">
                  <button
                    className="tb-btn btn-primary"
                    disabled={busy || !ownerPassword}
                    onClick={() => void submitProtect()}
                  >
                    {busy ? '保護中…' : '保護'}
                  </button>
                  <button className="tb-btn" onClick={onClose}>
                    取消
                  </button>
                </div>
              </>
            )}
          </div>
        )}
      </div>
    </div>
  )
}
