import { useState } from 'react'
import { decryptDocument } from '../api'

interface Props {
  id: string
  /** 上傳／開啟時已知的原檔名；未知時顯示通用文字。 */
  filename?: string
  onClose: () => void
}

/** 當文件上傳／開啟後 GET /info 失敗，且 GET /protection 回報 protected=true 時顯示：
 *  該文件是開檔密碼加密（Phase 12），本編輯器無法直接開啟／渲染。這裡只提供
 *  「輸入密碼→解密並下載」，不會建立可檢視的文件（解密結果同樣只下載、不進文件庫）。 */
export default function DecryptPrompt({ id, filename, onClose }: Props) {
  const [password, setPassword] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [done, setDone] = useState(false)

  const submit = async () => {
    if (!password) return
    setBusy(true)
    setError(null)
    try {
      await decryptDocument(id, password, filename ? `decrypted_${filename}` : undefined)
      setDone(true)
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
          <span>文件已加密</span>
          <button className="tb-btn" onClick={onClose}>
            ✕
          </button>
        </div>
        {done ? (
          <div className="modal-body">
            <p>解密完成，已下載可開啟的副本。本編輯器無法直接開啟含開檔密碼的 PDF，請改用解密後的檔案。</p>
            <div className="modal-footer">
              <button className="tb-btn btn-primary" onClick={onClose}>
                關閉
              </button>
            </div>
          </div>
        ) : (
          <div className="modal-body">
            <p>
              {filename ? `「${filename}」` : '此文件'}
              已加上開檔密碼保護，本編輯器無法直接開啟或顯示。請輸入密碼以解密並下載一份可開啟的副本。
            </p>
            {error && <div className="annot-hint">{error}</div>}

            <div className="modal-subtitle">密碼</div>
            <input
              type="password"
              className="modal-input"
              autoFocus
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') void submit()
              }}
            />

            <div className="modal-footer">
              <button
                className="tb-btn btn-primary"
                disabled={busy || !password}
                onClick={() => void submit()}
              >
                {busy ? '解密中…' : '解密並下載'}
              </button>
              <button className="tb-btn" onClick={onClose}>
                取消
              </button>
            </div>
          </div>
        )}
      </div>
    </div>
  )
}
