/**
 * Privacy layout — fetches storage stats, manages mutation states.
 * Child routes receive data via Outlet context.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { type ReactNode, useEffect, useId, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Outlet } from 'react-router-dom'
import {
  type BackupArchive,
  type BackupParams,
  type DeleteRangeRequest,
  type DeleteResult,
  deleteAllData,
  deleteDataRange,
  downloadBackup,
  downloadBlob,
  fetchStorageStats,
  type RestoreResult,
  restoreBackup,
  type StorageStats,
} from '../../api/client'
import { Alert, Button, Card, CardTitle, Spinner } from '../../components/ui'
import { addToast } from '../../hooks/useToast'
import { colors, elevation, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'

type DataType = 'events' | 'frames' | 'metrics' | 'processes' | 'idle'

interface ConfirmModalProps {
  isOpen: boolean
  title: string
  message: string
  confirmText: string
  isDangerous: boolean
  onConfirm: () => void
  onCancel: () => void
}

function ConfirmModal({ isOpen, title, message, confirmText, isDangerous, onConfirm, onCancel }: ConfirmModalProps) {
  const { t } = useTranslation()
  const dialogRef = useRef<HTMLDivElement>(null)
  const previousFocusRef = useRef<Element | null>(null)
  const descriptionId = useId()

  useEffect(() => {
    if (!isOpen) return
    previousFocusRef.current = document.activeElement

    const timer = setTimeout(() => {
      dialogRef.current?.querySelector<HTMLElement>('button')?.focus()
    }, 50)

    return () => {
      clearTimeout(timer)
      if (previousFocusRef.current instanceof HTMLElement) {
        previousFocusRef.current.focus()
      }
    }
  }, [isOpen])

  useEffect(() => {
    if (!isOpen) return
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        onCancel()
        return
      }
      if (e.key !== 'Tab') return
      const dialog = dialogRef.current
      if (!dialog) return
      const focusable = dialog.querySelectorAll<HTMLElement>(
        'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])',
      )
      if (focusable.length === 0) return
      const first = focusable[0]
      const last = focusable[focusable.length - 1]
      if (e.shiftKey) {
        if (document.activeElement === first) {
          e.preventDefault()
          last.focus()
        }
      } else {
        if (document.activeElement === last) {
          e.preventDefault()
          first.focus()
        }
      }
    }
    document.addEventListener('keydown', handleKeyDown)
    return () => document.removeEventListener('keydown', handleKeyDown)
  }, [isOpen, onCancel])

  if (!isOpen) return null

  return (
    <div className="fixed inset-0 z-dialog flex items-center justify-center bg-surface-overlay/50">
      <div ref={dialogRef} role="alertdialog" aria-modal="true" aria-describedby={descriptionId}>
        <Card variant="default" padding="lg" className={cn('mx-4 w-full max-w-md', elevation.dialog)}>
          <CardTitle className={`mb-2 ${isDangerous ? 'text-semantic-error' : ''}`}>{title}</CardTitle>
          <p id={descriptionId} className="mb-6 whitespace-pre-line text-content-secondary">
            {message}
          </p>
          <div className="flex justify-end space-x-3">
            <Button variant="secondary" onClick={onCancel}>
              {t('privacy.cancel')}
            </Button>
            <Button variant={isDangerous ? 'danger' : 'primary'} onClick={onConfirm}>
              {confirmText}
            </Button>
          </div>
        </Card>
      </div>
    </div>
  )
}

export interface PrivacyContext {
  storageStats: StorageStats | undefined
  deleteResult: DeleteResult | null
  setDeleteResult: React.Dispatch<React.SetStateAction<DeleteResult | null>>
  deleteRangeMutation: ReturnType<typeof useMutation<DeleteResult, Error, DeleteRangeRequest>>
  deleteAllMutation: ReturnType<typeof useMutation<DeleteResult, Error, void>>
  backupMutation: ReturnType<typeof useMutation<Blob, Error, BackupParams | undefined>>
  restoreMutation: ReturnType<typeof useMutation<RestoreResult, Error, BackupArchive>>
  restoreResult: RestoreResult | null
  setRestoreResult: React.Dispatch<React.SetStateAction<RestoreResult | null>>
  restoreError: string | null
  setRestoreError: React.Dispatch<React.SetStateAction<string | null>>
  backupOptions: BackupParams
  setBackupOptions: React.Dispatch<React.SetStateAction<BackupParams>>
  fileInputRef: React.RefObject<HTMLInputElement>
  handleBackup: () => void
  handleRestoreFile: (e: React.ChangeEvent<HTMLInputElement>) => Promise<void>
  showDeleteRangeModal: boolean
  setShowDeleteRangeModal: React.Dispatch<React.SetStateAction<boolean>>
  showDeleteAllModal: boolean
  setShowDeleteAllModal: React.Dispatch<React.SetStateAction<boolean>>
  fromDate: string
  setFromDate: React.Dispatch<React.SetStateAction<string>>
  toDate: string
  setToDate: React.Dispatch<React.SetStateAction<string>>
  selectedDataTypes: DataType[]
  handleDataTypeToggle: (type: DataType) => void
  handleDeleteRange: () => void
  handleDeleteAll: () => void
  DATA_TYPE_LABELS: Record<DataType, string>
  getDateRangeText: () => string
}

export default function PrivacyLayout() {
  const { t } = useTranslation()
  const DATA_TYPE_LABELS: Record<DataType, string> = {
    events: t('privacy.dataTypes.events'),
    frames: t('privacy.dataTypes.frames'),
    metrics: t('privacy.dataTypes.metrics'),
    processes: t('privacy.dataTypes.process_snapshots'),
    idle: t('privacy.dataTypes.idle_periods'),
  }
  const queryClient = useQueryClient()
  const fileInputRef = useRef<HTMLInputElement>(null)
  const [showDeleteRangeModal, setShowDeleteRangeModal] = useState(false)
  const [showDeleteAllModal, setShowDeleteAllModal] = useState(false)
  const [deleteResult, setDeleteResult] = useState<DeleteResult | null>(null)

  const [fromDate, setFromDate] = useState('')
  const [toDate, setToDate] = useState('')
  const [selectedDataTypes, setSelectedDataTypes] = useState<DataType[]>([])

  const [backupOptions, setBackupOptions] = useState<BackupParams>({
    include_settings: true,
    include_tags: true,
    include_events: false,
    include_frames: false,
  })
  const [restoreResult, setRestoreResult] = useState<RestoreResult | null>(null)
  const [restoreError, setRestoreError] = useState<string | null>(null)

  const { data: storageStats, isLoading } = useQuery({
    queryKey: ['storage-stats'],
    queryFn: fetchStorageStats,
  })

  const deleteRangeMutation = useMutation({
    mutationFn: deleteDataRange,
    onSuccess: (result) => {
      setDeleteResult(result)
      addToast('success', result.message)
      queryClient.invalidateQueries({ queryKey: ['storage-stats'] })
      queryClient.invalidateQueries({ queryKey: ['frames'] })
      queryClient.invalidateQueries({ queryKey: ['metrics'] })
      setShowDeleteRangeModal(false)
      setFromDate('')
      setToDate('')
      setSelectedDataTypes([])
    },
    onError: (error: Error) => {
      addToast('error', error.message)
    },
  })

  const deleteAllMutation = useMutation({
    mutationFn: deleteAllData,
    onSuccess: (result) => {
      setDeleteResult(result)
      addToast('success', result.message)
      queryClient.invalidateQueries()
      setShowDeleteAllModal(false)
    },
    onError: (error: Error) => {
      addToast('error', error.message)
    },
  })

  const backupMutation = useMutation({
    mutationFn: downloadBackup,
    onSuccess: (blob) => {
      const now = new Date().toISOString().slice(0, 19).replace(/[-:]/g, '').replace('T', '_')
      downloadBlob(blob, `oneshim_backup_${now}.json`)
      addToast('success', t('backup.downloadComplete'))
    },
    onError: (error: Error) => {
      addToast('error', `${t('backup.downloadFailed')}: ${error.message}`)
    },
  })

  const restoreMutation = useMutation({
    mutationFn: restoreBackup,
    onSuccess: (result) => {
      setRestoreResult(result)
      addToast(
        result.success ? 'success' : 'error',
        result.success ? t('backup.restoreSuccess') : t('backup.restoreFailed'),
      )
      queryClient.invalidateQueries()
    },
    onError: (error: Error) => {
      addToast('error', `${t('backup.restoreFailed')}: ${error.message}`)
    },
  })

  const handleDataTypeToggle = (type: DataType) => {
    setSelectedDataTypes((prev) => (prev.includes(type) ? prev.filter((dt) => dt !== type) : [...prev, type]))
  }

  const handleDeleteRange = () => {
    if (!fromDate || !toDate) return

    const request: DeleteRangeRequest = {
      from: fromDate,
      to: toDate,
      data_types: selectedDataTypes.length > 0 ? selectedDataTypes : undefined,
    }

    deleteRangeMutation.mutate(request)
  }

  const handleDeleteAll = () => {
    deleteAllMutation.mutate()
  }

  const handleBackup = () => {
    backupMutation.mutate(backupOptions)
  }

  const handleRestoreFile = async (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0]
    if (!file) return

    setRestoreError(null)

    try {
      const text = await file.text()
      const archive: BackupArchive = JSON.parse(text)

      if (!archive.metadata?.version) {
        setRestoreError(t('backup.restoreFailed'))
        return
      }

      restoreMutation.mutate(archive)
    } catch {
      setRestoreError(t('backup.restoreFailed'))
    }

    if (fileInputRef.current) {
      fileInputRef.current.value = ''
    }
  }

  const getDateRangeText = () => {
    if (storageStats?.oldest_data_date && storageStats?.newest_data_date) {
      return `${storageStats.oldest_data_date.split('T')[0]} ~ ${storageStats.newest_data_date.split('T')[0]}`
    }
    return t('common.noData')
  }

  if (isLoading) {
    return (
      <div className="flex h-64 items-center justify-center">
        <Spinner size="lg" className="text-brand-text" />
        <span className="ml-3 text-content-secondary">{t('common.loading')}</span>
      </div>
    )
  }

  const ctx: PrivacyContext = {
    storageStats,
    deleteResult,
    setDeleteResult,
    deleteRangeMutation,
    deleteAllMutation,
    backupMutation,
    restoreMutation,
    restoreResult,
    setRestoreResult,
    restoreError,
    setRestoreError,
    backupOptions,
    setBackupOptions,
    fileInputRef,
    handleBackup,
    handleRestoreFile,
    showDeleteRangeModal,
    setShowDeleteRangeModal,
    showDeleteAllModal,
    setShowDeleteAllModal,
    fromDate,
    setFromDate,
    toDate,
    setToDate,
    selectedDataTypes,
    handleDataTypeToggle,
    handleDeleteRange,
    handleDeleteAll,
    DATA_TYPE_LABELS,
    getDateRangeText,
  }

  return (
    <div className="min-h-full space-y-6 p-6">
      {/* Header */}
      <div>
        <h1 className={cn(typography.h1, colors.text.pageTitle)}>{t('privacy.title')}</h1>
        <p className={cn('mt-1', colors.text.pageSubtitle)}>{t('privacy.subtitle')}</p>
      </div>

      {/* Delete result alert */}
      {deleteResult && (
        <Alert variant="success" title={deleteResult.message}>
          <div className="mt-2 space-y-1 text-sm">
            {deleteResult.events_deleted > 0 && (
              <div>{t('privacy.deleteResult.events', { count: deleteResult.events_deleted })}</div>
            )}
            {deleteResult.frames_deleted > 0 && (
              <div>{t('privacy.deleteResult.frames', { count: deleteResult.frames_deleted })}</div>
            )}
            {deleteResult.metrics_deleted > 0 && (
              <div>{t('privacy.deleteResult.metrics', { count: deleteResult.metrics_deleted })}</div>
            )}
            {deleteResult.process_snapshots_deleted > 0 && (
              <div>
                {t('privacy.deleteResult.process_snapshots', { count: deleteResult.process_snapshots_deleted })}
              </div>
            )}
            {deleteResult.idle_periods_deleted > 0 && (
              <div>{t('privacy.deleteResult.idle_periods', { count: deleteResult.idle_periods_deleted })}</div>
            )}
          </div>
          <button
            type="button"
            onClick={() => setDeleteResult(null)}
            className="mt-3 text-sm underline hover:no-underline"
          >
            {t('privacy.close')}
          </button>
        </Alert>
      )}

      <Outlet context={ctx} />

      {/* Data info card */}
      <Card variant="default" padding="lg">
        <CardTitle className="mb-4">{t('privacy.dataInfoTitle')}</CardTitle>
        <div className="space-y-3 text-content-strong text-sm">
          <div className="flex items-start space-x-2">
            <span className="text-brand-text">✓</span>
            <span>{t('privacy.dataInfo1')}</span>
          </div>
          <div className="flex items-start space-x-2">
            <span className="text-brand-text">✓</span>
            <span>{t('privacy.dataInfo2')}</span>
          </div>
          <div className="flex items-start space-x-2">
            <span className="text-brand-text">✓</span>
            <span>{t('privacy.dataInfo3')}</span>
          </div>
          <div className="flex items-start space-x-2">
            <span className="text-brand-text">✓</span>
            <span>{t('privacy.dataInfo4')}</span>
          </div>
        </div>
      </Card>

      {/* Confirm modals */}
      <ConfirmModal
        isOpen={showDeleteRangeModal}
        title={t('privacy.confirmDeleteRange')}
        message={t('privacy.confirmDeleteRangeMsg', {
          fromDate,
          toDate,
          dataTypes:
            selectedDataTypes.length > 0
              ? selectedDataTypes.map((dt) => DATA_TYPE_LABELS[dt]).join(', ')
              : t('privacy.allDataTypes', 'All data types'),
          defaultValue:
            'Delete data from {{fromDate}} to {{toDate}}.\n\nTarget: {{dataTypes}}\n\nThis action cannot be undone.',
        })}
        confirmText={t('privacy.deleteRange')}
        isDangerous={false}
        onConfirm={handleDeleteRange}
        onCancel={() => setShowDeleteRangeModal(false)}
      />

      <ConfirmModal
        isOpen={showDeleteAllModal}
        title={t('privacy.confirmDeleteAll')}
        message={t('privacy.confirmDeleteAllMsg')}
        confirmText={t('privacy.deleteAllButton')}
        isDangerous={true}
        onConfirm={handleDeleteAll}
        onCancel={() => setShowDeleteAllModal(false)}
      />
    </div>
  )
}

interface DataCardProps {
  label: string
  value: string
  icon: ReactNode
  small?: boolean
}

export function DataCard({ label, value, icon, small }: DataCardProps) {
  return (
    <Card variant="elevated" padding="md">
      <div className="flex items-center space-x-2 text-content-secondary text-sm">
        {icon}
        <span>{label}</span>
      </div>
      <div className={`mt-1 ${typography.weight.bold} text-content ${small ? 'text-sm' : 'text-xl'}`}>{value}</div>
    </Card>
  )
}
