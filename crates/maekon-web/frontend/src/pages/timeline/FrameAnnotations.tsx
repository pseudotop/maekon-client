/**
 * #8078 (CJ-02-04): user annotation add/delete for a captured frame.
 *
 * A lean note lifecycle for the timeline detail panel — list existing notes,
 * add a new one, delete an existing one. Notes are local-only (no cross-device
 * sync) and persist in the V30 `frame_annotations` table via
 * `handlers::annotations`. Positional highlight annotations (x/y/width/height)
 * are not authored here; a note is stored as a `Memo` at the origin.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Trash2 } from 'lucide-react'
import { useCallback, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { createFrameAnnotation, deleteFrameAnnotation, fetchFrameAnnotations } from '../../api/client'
import { Button, Input } from '../../components/ui'
import { useCaptureMutationRecovery } from '../../hooks/useCaptureMutationRecovery'
import { iconSize, typography } from '../../styles/tokens'
import { formatDate, formatTime } from '../../utils/formatters'

interface FrameAnnotationsProps {
  frameId: number
}

export function FrameAnnotations({ frameId }: FrameAnnotationsProps) {
  const { t, i18n } = useTranslation()
  const locale = i18n.resolvedLanguage ?? i18n.language
  const queryClient = useQueryClient()
  const [text, setText] = useState('')
  const recoverMutation = useCaptureMutationRecovery(t('timeline.annotationUpdateFailed'))

  const { data: annotations = [] } = useQuery({
    queryKey: ['frame-annotations', frameId],
    queryFn: () => fetchFrameAnnotations(frameId),
  })

  const invalidate = useCallback(
    () => queryClient.invalidateQueries({ queryKey: ['frame-annotations', frameId] }),
    [frameId, queryClient],
  )

  const completeAdd = useCallback(async () => {
    setText('')
    await invalidate()
  }, [invalidate])

  const addMutation = useMutation({
    mutationFn: (note: string) => createFrameAnnotation(frameId, { annotation_type: 'Memo', x: 0, y: 0, text: note }),
    onSuccess: completeAdd,
    onError: (error, note) => {
      void recoverMutation(error, async () => {
        await createFrameAnnotation(frameId, { annotation_type: 'Memo', x: 0, y: 0, text: note })
        await completeAdd()
      })
    },
  })

  const deleteMutation = useMutation({
    mutationFn: (annotationId: string) => deleteFrameAnnotation(frameId, annotationId),
    onSuccess: invalidate,
    onError: (error, annotationId) => {
      void recoverMutation(error, async () => {
        await deleteFrameAnnotation(frameId, annotationId)
        await invalidate()
      })
    },
  })

  const submit = () => {
    const trimmed = text.trim()
    if (trimmed) addMutation.mutate(trimmed)
  }

  return (
    <div>
      <h4 className={`mb-2 ${typography.weight.medium} text-content-secondary text-sm`}>
        {t('timeline.annotations', 'Notes')}
      </h4>
      <div className="space-y-2">
        {annotations.length > 0 && (
          <ul className="space-y-1">
            {annotations.map((annotation) => (
              <li
                key={annotation.annotation_id}
                className="flex items-start justify-between gap-2 rounded bg-surface-muted px-2 py-1.5"
              >
                <div className="min-w-0">
                  <p className="whitespace-pre-wrap break-words text-content text-sm">{annotation.text}</p>
                  <span className="text-content-tertiary text-xs">
                    {formatDate(annotation.created_at, locale)} {formatTime(annotation.created_at, locale)}
                  </span>
                </div>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  aria-label={t('timeline.deleteAnnotation', 'Delete note')}
                  isLoading={deleteMutation.isPending && deleteMutation.variables === annotation.annotation_id}
                  onClick={() => deleteMutation.mutate(annotation.annotation_id)}
                >
                  <Trash2 className={iconSize.base} />
                </Button>
              </li>
            ))}
          </ul>
        )}
        <form
          className="flex items-center gap-2"
          onSubmit={(event) => {
            event.preventDefault()
            submit()
          }}
        >
          <Input
            value={text}
            onChange={(event) => setText(event.target.value)}
            placeholder={t('timeline.addAnnotation', 'Add a note...')}
            inputSize="sm"
          />
          <Button type="submit" variant="secondary" size="sm" isLoading={addMutation.isPending} disabled={!text.trim()}>
            {t('timeline.addAnnotationSubmit', 'Add')}
          </Button>
        </form>
      </div>
    </div>
  )
}
