import { useEffect, useMemo, useRef, useState } from 'react'
import {
  actionRunDownloadUrl,
  actionRunFileUrl,
  createAction,
  listActions,
  listDocuments,
  pollActionRun,
  runAction,
  uploadPdf,
  type ActionDef,
  type ActionRunStatus,
  type DocMeta,
  type Step,
  type StepSecrets,
} from '../api'
import StepEditor, { defaultStepForType, STEP_TYPE_LABELS, STEP_TYPE_ORDER } from './StepEditor'

interface Props {
  onClose: () => void
  onOpenDoc: (id: string) => void | Promise<void>
}

type WizardPhase = 'pipeline' | 'files' | 'secrets' | 'run'

interface PendingFile {
  file: File
  status: 'pending' | 'uploading' | 'done' | 'error'
  id?: string
  error?: string
}

const POLL_INTERVAL_MS = 700

function validateSteps(steps: Step[]): string | null {
  if (steps.length === 0) return '請至少加入一個動作'
  const exportIndex = steps.findIndex((s) => s.type === 'export')
  if (exportIndex !== -1 && exportIndex !== steps.length - 1) {
    return `匯出（export）必須是最後一步（目前在第 ${exportIndex + 1} 步）`
  }
  return null
}

function stepNeedsSecret(step: Step): boolean {
  return step.type === 'protect' || step.type === 'encrypt'
}

function summarizeStep(step: Step): string {
  return STEP_TYPE_LABELS[step.type]
}

export default function ActionWizardDialog({ onClose, onOpenDoc }: Props) {
  const [phase, setPhase] = useState<WizardPhase>('pipeline')

  // ---- Step 1：選擇/建立 pipeline ----
  const [pipelineMode, setPipelineMode] = useState<'existing' | 'new'>('existing')
  const [existingActions, setExistingActions] = useState<ActionDef[]>([])
  const [selectedExistingId, setSelectedExistingId] = useState<string | null>(null)
  const [newName, setNewName] = useState('')
  const [newSteps, setNewSteps] = useState<Step[]>([])
  const [savedAction, setSavedAction] = useState<ActionDef | null>(null)
  const [pipelineError, setPipelineError] = useState<string | null>(null)
  const [pipelineBusy, setPipelineBusy] = useState(false)

  useEffect(() => {
    listActions()
      .then(setExistingActions)
      .catch((err) => setPipelineError(err instanceof Error ? err.message : String(err)))
  }, [])

  // ---- Step 2：選檔案（既有文件 + 上傳新檔） ----
  const [allDocs, setAllDocs] = useState<DocMeta[]>([])
  const [selectedDocIds, setSelectedDocIds] = useState<string[]>([])
  const [pendingFiles, setPendingFiles] = useState<PendingFile[]>([])
  const [dragOver, setDragOver] = useState(false)
  const fileInputRef = useRef<HTMLInputElement>(null)
  const folderInputRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    if (phase !== 'files') return
    listDocuments()
      .then(setAllDocs)
      .catch(() => {
        // 既有文件清單抓不到就只留上傳新檔這條路，不擋住整個 wizard
      })
  }, [phase])

  const uploadFiles = (files: File[]) => {
    const pdfFiles = files.filter((f) => f.type === 'application/pdf' || f.name.toLowerCase().endsWith('.pdf'))
    if (pdfFiles.length === 0) return
    const startIndex = pendingFiles.length
    setPendingFiles((prev) => [...prev, ...pdfFiles.map((file): PendingFile => ({ file, status: 'pending' }))])
    pdfFiles.forEach((file, i) => {
      const idx = startIndex + i
      setPendingFiles((prev) => prev.map((p, pi) => (pi === idx ? { ...p, status: 'uploading' } : p)))
      uploadPdf(file)
        .then(({ id }) => {
          setPendingFiles((prev) => prev.map((p, pi) => (pi === idx ? { ...p, status: 'done', id } : p)))
        })
        .catch((err) => {
          const message = err instanceof Error ? err.message : String(err)
          setPendingFiles((prev) => prev.map((p, pi) => (pi === idx ? { ...p, status: 'error', error: message } : p)))
        })
    })
  }

  const toggleDoc = (id: string) => {
    setSelectedDocIds((s) => (s.includes(id) ? s.filter((x) => x !== id) : [...s, id]))
  }

  const removePendingFile = (i: number) => setPendingFiles((prev) => prev.filter((_, pi) => pi !== i))

  const uploadedIds = pendingFiles.filter((p) => p.status === 'done').map((p) => p.id!)
  const documentIds = useMemo(() => [...selectedDocIds, ...uploadedIds], [selectedDocIds, uploadedIds])
  const uploadsInFlight = pendingFiles.some((p) => p.status === 'pending' || p.status === 'uploading')

  const labelForDocId = (id: string): string =>
    allDocs.find((d) => d.id === id)?.filename ?? pendingFiles.find((p) => p.id === id)?.file.name ?? id

  // ---- Step 3：需要密碼的 step 的 run-time secrets ----
  const [stepSecrets, setStepSecrets] = useState<Record<number, StepSecrets>>({})
  const secretSteps = useMemo(
    () => (savedAction ? savedAction.steps.map((s, i) => ({ step: s, index: i })).filter(({ step }) => stepNeedsSecret(step)) : []),
    [savedAction],
  )

  const setSecret = (index: number, patch: Partial<StepSecrets>) => {
    setStepSecrets((prev) => ({ ...prev, [index]: { ...prev[index], ...patch } }))
  }

  const secretsMissing = secretSteps.some(({ step, index }) => {
    const secret = stepSecrets[index]
    if (step.type === 'protect') return !secret?.owner_password
    if (step.type === 'encrypt') return !secret?.user_password
    return false
  })

  // ---- Step 4：執行 + 進度 ----
  const [runId, setRunId] = useState<string | null>(null)
  const [runStatus, setRunStatus] = useState<ActionRunStatus | null>(null)
  const [runError, setRunError] = useState<string | null>(null)
  const [running, setRunning] = useState(false)
  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null)

  useEffect(() => {
    return () => {
      if (pollRef.current) clearInterval(pollRef.current)
    }
  }, [])

  const submitPipeline = async () => {
    if (pipelineMode === 'existing') {
      const action = existingActions.find((a) => a.id === selectedExistingId)
      if (!action) {
        setPipelineError('請選擇一個 pipeline')
        return
      }
      setSavedAction(action)
      setPhase('files')
      return
    }
    const err = validateSteps(newSteps)
    if (err) {
      setPipelineError(err)
      return
    }
    if (!newName.trim()) {
      setPipelineError('請輸入 pipeline 名稱')
      return
    }
    setPipelineBusy(true)
    setPipelineError(null)
    try {
      const def = await createAction(newName.trim(), newSteps)
      setSavedAction(def)
      setPhase('files')
    } catch (createErr) {
      setPipelineError(createErr instanceof Error ? createErr.message : String(createErr))
    } finally {
      setPipelineBusy(false)
    }
  }

  const goToConfirm = () => {
    if (secretSteps.length > 0) {
      setPhase('secrets')
    } else {
      setPhase('run')
    }
  }

  const startRun = async () => {
    if (!savedAction) return
    setRunning(true)
    setRunError(null)
    try {
      const { run_id } = await runAction(savedAction.id, {
        documentIds,
        stepSecrets: secretSteps.length > 0 ? stepSecrets : undefined,
      })
      setRunId(run_id)
      pollRef.current = setInterval(() => {
        pollActionRun(run_id)
          .then((status) => {
            setRunStatus(status)
            if (status.status === 'done') {
              if (pollRef.current) {
                clearInterval(pollRef.current)
                pollRef.current = null
              }
              setRunning(false)
            }
          })
          .catch((pollErr) => {
            if (pollRef.current) {
              clearInterval(pollRef.current)
              pollRef.current = null
            }
            setRunError(pollErr instanceof Error ? pollErr.message : String(pollErr))
            setRunning(false)
          })
      }, POLL_INTERVAL_MS)
    } catch (runErr) {
      setRunError(runErr instanceof Error ? runErr.message : String(runErr))
      setRunning(false)
    }
  }

  const addStep = (type: Step['type']) => setNewSteps((s) => [...s, defaultStepForType(type)])
  const updateStep = (i: number, step: Step) => setNewSteps((s) => s.map((x, xi) => (xi === i ? step : x)))
  const removeStep = (i: number) => setNewSteps((s) => s.filter((_, xi) => xi !== i))
  const moveStep = (i: number, dir: -1 | 1) => {
    setNewSteps((s) => {
      const j = i + dir
      if (j < 0 || j >= s.length) return s
      const next = [...s]
      ;[next[i], next[j]] = [next[j], next[i]]
      return next
    })
  }

  const hasExportStep = newSteps.some((s) => s.type === 'export')

  return (
    <div
      className="modal-overlay"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose()
      }}
    >
      <div className="modal modal-wide">
        <div className="modal-header">
          <span>動作精靈（批次執行）</span>
          <button className="tb-btn" onClick={onClose}>
            ✕
          </button>
        </div>

        <div className="wizard-steps" style={{ padding: '0 12px' }}>
          {(['pipeline', 'files', 'secrets', 'run'] as WizardPhase[])
            .filter((p) => p !== 'secrets' || secretSteps.length > 0 || phase === 'secrets')
            .map((p, i) => (
              <div key={p} className={`wizard-step-tab ${phase === p ? 'active' : ''} ${phase !== p ? 'done' : ''}`}>
                {i + 1}. {{ pipeline: '選擇動作', files: '選檔案', secrets: '密碼', run: '執行' }[p]}
              </div>
            ))}
        </div>

        {phase === 'pipeline' && (
          <div className="modal-body">
            {pipelineError && <div className="annot-hint">{pipelineError}</div>}
            <div className="wizard-mode-tabs">
              <button className={`tb-btn ${pipelineMode === 'existing' ? 'active' : ''}`} onClick={() => setPipelineMode('existing')}>
                使用已存 pipeline
              </button>
              <button className={`tb-btn ${pipelineMode === 'new' ? 'active' : ''}`} onClick={() => setPipelineMode('new')}>
                建立新 pipeline
              </button>
            </div>

            {pipelineMode === 'existing' ? (
              <div className="doc-list">
                {existingActions.length === 0 && <div className="modal-subtitle">目前沒有已存的 pipeline</div>}
                {existingActions.map((a) => (
                  <label key={a.id} className="doc-list-item">
                    <input
                      type="radio"
                      name="existing-action"
                      checked={selectedExistingId === a.id}
                      onChange={() => setSelectedExistingId(a.id)}
                    />
                    <span className="doc-list-name">
                      {a.name}（{a.steps.map(summarizeStep).join(' → ')}）
                    </span>
                  </label>
                ))}
              </div>
            ) : (
              <>
                <div className="modal-subtitle">Pipeline 名稱</div>
                <input className="modal-input" value={newName} onChange={(e) => setNewName(e.target.value)} />

                <div className="modal-subtitle">動作序列</div>
                {newSteps.map((step, i) => (
                  <div key={i} className="wizard-step-card">
                    <div className="wizard-step-card-header">
                      <span>#{i + 1}</span>
                      <select
                        value={step.type}
                        onChange={(e) => updateStep(i, defaultStepForType(e.target.value as Step['type']))}
                      >
                        {STEP_TYPE_ORDER.map((t) => (
                          <option key={t} value={t}>
                            {STEP_TYPE_LABELS[t]}
                          </option>
                        ))}
                      </select>
                      <button className="tb-btn" disabled={i === 0} onClick={() => moveStep(i, -1)}>
                        ↑
                      </button>
                      <button className="tb-btn" disabled={i === newSteps.length - 1} onClick={() => moveStep(i, 1)}>
                        ↓
                      </button>
                      <button className="tb-btn" onClick={() => removeStep(i)}>
                        移除
                      </button>
                    </div>
                    <StepEditor step={step} onChange={(s) => updateStep(i, s)} />
                  </div>
                ))}

                <div className="modal-footer" style={{ justifyContent: 'flex-start' }}>
                  <select
                    className="modal-input"
                    disabled={hasExportStep}
                    value=""
                    onChange={(e) => {
                      if (e.target.value) addStep(e.target.value as Step['type'])
                    }}
                  >
                    <option value="">+ 新增動作…</option>
                    {STEP_TYPE_ORDER.map((t) => (
                      <option key={t} value={t}>
                        {STEP_TYPE_LABELS[t]}
                      </option>
                    ))}
                  </select>
                </div>
                {hasExportStep && <div className="modal-subtitle">匯出已是最後一步，不能再加動作</div>}
              </>
            )}

            <div className="modal-footer">
              <button className="tb-btn btn-primary" disabled={pipelineBusy} onClick={() => void submitPipeline()}>
                {pipelineBusy ? '建立中…' : '下一步'}
              </button>
              <button className="tb-btn" onClick={onClose}>
                取消
              </button>
            </div>
          </div>
        )}

        {phase === 'files' && (
          <div className="modal-body">
            <div className="modal-subtitle">從已上傳文件選擇</div>
            <div className="doc-list">
              {allDocs.length === 0 && <div className="modal-subtitle">目前沒有已上傳的文件</div>}
              {allDocs.map((d) => (
                <label key={d.id} className="doc-list-item">
                  <input type="checkbox" checked={selectedDocIds.includes(d.id)} onChange={() => toggleDoc(d.id)} />
                  <span className="doc-list-name">{d.filename}</span>
                </label>
              ))}
            </div>

            <div className="modal-subtitle">或上傳新檔案</div>
            <div
              className={`wizard-dropzone ${dragOver ? 'drag-over' : ''}`}
              onDragOver={(e) => {
                e.preventDefault()
                setDragOver(true)
              }}
              onDragLeave={() => setDragOver(false)}
              onDrop={(e) => {
                e.preventDefault()
                setDragOver(false)
                uploadFiles(Array.from(e.dataTransfer.files))
              }}
            >
              拖曳 PDF 檔案到這裡
              <div className="modal-footer" style={{ justifyContent: 'center', marginTop: 8 }}>
                <button className="tb-btn" onClick={() => fileInputRef.current?.click()}>
                  選擇檔案
                </button>
                <button className="tb-btn" onClick={() => folderInputRef.current?.click()}>
                  選擇資料夾
                </button>
              </div>
            </div>
            <input
              ref={fileInputRef}
              type="file"
              accept="application/pdf"
              multiple
              hidden
              onChange={(e) => {
                uploadFiles(Array.from(e.target.files ?? []))
                e.target.value = ''
              }}
            />
            <input
              ref={folderInputRef}
              type="file"
              multiple
              hidden
              // @ts-expect-error webkitdirectory 沒有正式型別，但 Chromium(含 Tauri webview2) 支援
              webkitdirectory=""
              onChange={(e) => {
                uploadFiles(Array.from(e.target.files ?? []))
                e.target.value = ''
              }}
            />

            {pendingFiles.length > 0 && (
              <div className="doc-order-list">
                {pendingFiles.map((p, i) => (
                  <div key={i} className="wizard-file-row">
                    <span className="wizard-file-row-name">{p.file.name}</span>
                    <span className={`wizard-file-status-${p.status === 'error' ? 'error' : p.status === 'done' ? 'done' : 'pending'}`}>
                      {p.status === 'pending' && '等待上傳'}
                      {p.status === 'uploading' && '上傳中…'}
                      {p.status === 'done' && '已上傳'}
                      {p.status === 'error' && (p.error ?? '上傳失敗')}
                    </span>
                    <button className="tb-btn" onClick={() => removePendingFile(i)}>
                      移除
                    </button>
                  </div>
                ))}
              </div>
            )}

            <div className="modal-footer">
              <button
                className="tb-btn btn-primary"
                disabled={documentIds.length === 0 || uploadsInFlight}
                onClick={goToConfirm}
              >
                下一步（已選 {documentIds.length} 份）
              </button>
              <button className="tb-btn" onClick={() => setPhase('pipeline')}>
                上一步
              </button>
            </div>
          </div>
        )}

        {phase === 'secrets' && (
          <div className="modal-body">
            <div className="annot-hint">密碼只在這次執行送出，不會存進 pipeline 定義。</div>
            {secretSteps.map(({ step, index }) => (
              <div key={index} className="wizard-step-card">
                <div className="modal-subtitle">
                  第 {index + 1} 步：{STEP_TYPE_LABELS[step.type]}
                </div>
                {step.type === 'protect' && (
                  <input
                    type="password"
                    className="modal-input"
                    placeholder="擁有者密碼"
                    value={stepSecrets[index]?.owner_password ?? ''}
                    onChange={(e) => setSecret(index, { owner_password: e.target.value })}
                  />
                )}
                {step.type === 'encrypt' && (
                  <>
                    <input
                      type="password"
                      className="modal-input"
                      placeholder="開檔密碼"
                      value={stepSecrets[index]?.user_password ?? ''}
                      onChange={(e) => setSecret(index, { user_password: e.target.value })}
                    />
                    <input
                      type="password"
                      className="modal-input"
                      placeholder="擁有者密碼（可選，預設同開檔密碼）"
                      value={stepSecrets[index]?.owner_password ?? ''}
                      onChange={(e) => setSecret(index, { owner_password: e.target.value })}
                    />
                  </>
                )}
              </div>
            ))}
            <div className="modal-footer">
              <button className="tb-btn btn-primary" disabled={secretsMissing} onClick={() => setPhase('run')}>
                下一步
              </button>
              <button className="tb-btn" onClick={() => setPhase('files')}>
                上一步
              </button>
            </div>
          </div>
        )}

        {phase === 'run' && savedAction && (
          <div className="modal-body">
            {runError && <div className="annot-hint">{runError}</div>}
            {!runId && (
              <>
                <div className="modal-subtitle">
                  Pipeline：{savedAction.name}（{savedAction.steps.map(summarizeStep).join(' → ')}）
                </div>
                <div className="modal-subtitle">共 {documentIds.length} 份文件</div>
                <div className="modal-footer">
                  <button className="tb-btn btn-primary" disabled={running} onClick={() => void startRun()}>
                    {running ? '啟動中…' : '開始執行'}
                  </button>
                  <button className="tb-btn" onClick={() => setPhase(secretSteps.length > 0 ? 'secrets' : 'files')}>
                    上一步
                  </button>
                </div>
              </>
            )}

            {runId && (
              <>
                <div className="doc-order-list">
                  {documentIds.map((id, i) => {
                    const done = runStatus?.status === 'done'
                    const result = done ? runStatus.results.find((r) => r.source_document_id === id) : undefined
                    const runningIdx = runStatus?.status === 'running' ? runStatus.current_file : -1
                    let statusLabel: string
                    if (done && result) {
                      statusLabel =
                        result.outcome === 'failed'
                          ? `失敗：${result.message}`
                          : result.outcome === 'exported'
                            ? '完成（可下載）'
                            : '完成'
                    } else if (i < runningIdx) {
                      statusLabel = '已處理'
                    } else if (i === runningIdx && runStatus?.status === 'running') {
                      statusLabel = `處理中（第 ${runStatus.current_step + 1}/${runStatus.total_steps} 步）`
                    } else {
                      statusLabel = '等待中'
                    }
                    return (
                      <div key={id} className="wizard-file-row">
                        <span className="wizard-file-row-name">{labelForDocId(id)}</span>
                        <span
                          className={
                            result?.outcome === 'failed'
                              ? 'wizard-file-status-error'
                              : done
                                ? 'wizard-file-status-done'
                                : 'wizard-file-status-pending'
                          }
                        >
                          {statusLabel}
                        </span>
                        {result?.outcome === 'exported' && (
                          <a className="tb-btn" href={actionRunFileUrl(runId, result.index)} download={result.filename}>
                            下載
                          </a>
                        )}
                        {result?.outcome === 'document' && (
                          <button className="tb-btn" onClick={() => void onOpenDoc(result.document.id)}>
                            開啟
                          </button>
                        )}
                      </div>
                    )
                  })}
                </div>

                {runStatus?.status === 'done' && runStatus.results.some((r) => r.outcome === 'exported') && (
                  <div className="modal-footer" style={{ justifyContent: 'center' }}>
                    <a className="tb-btn btn-primary" href={actionRunDownloadUrl(runId)}>
                      下載全部（zip）
                    </a>
                  </div>
                )}

                <div className="modal-footer">
                  <button className="tb-btn" onClick={onClose}>
                    關閉
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
