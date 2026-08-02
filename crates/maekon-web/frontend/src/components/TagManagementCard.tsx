import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Tag as TagIcon } from 'lucide-react'
import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { deleteTag, fetchTags, TAGS_QUERY_KEY, updateTag } from '../api/client'
import type { Tag } from '../api/contracts'
import { colors, iconSize, motion } from '../styles/tokens'
import { cn } from '../utils/cn'
import { TAG_COLORS } from './TagBadge'
import {
  Alert,
  Button,
  Card,
  CardTitle,
  Dialog,
  DialogBody,
  DialogContent,
  DialogFooter,
  DialogTitle,
  EmptyState,
  Input,
  Spinner,
} from './ui'

/**
 * Settings surface for renaming and deleting saved tags (#9691).
 *
 * `updateTag` / `deleteTag` shipped in the API client with no caller, so tags
 * created from the timeline could never be corrected or removed. Both mutations
 * invalidate the shared `['tags']` query key, so the timeline filter and the
 * search facet pick the change up without a reload.
 */
export function TagManagementCard() {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const [editing, setEditing] = useState<Tag | null>(null)
  const [deleting, setDeleting] = useState<Tag | null>(null)
  const [draftName, setDraftName] = useState('')
  const [draftColor, setDraftColor] = useState<string>(TAG_COLORS[0])
  const [actionError, setActionError] = useState<string | null>(null)

  const { data: tags = [], isLoading } = useQuery({
    queryKey: TAGS_QUERY_KEY,
    queryFn: fetchTags,
  })

  // Seed the draft from whichever tag the user opened, not from render state —
  // reopening the dialog on a different tag must not keep the previous name.
  useEffect(() => {
    if (editing) {
      setDraftName(editing.name)
      setDraftColor(editing.color)
      setActionError(null)
    }
  }, [editing])

  // Closing a dialog must also discard its error. Otherwise the failure the user
  // just walked away from re-renders as an undismissable banner on the card.
  const closeEditing = () => {
    setEditing(null)
    setActionError(null)
  }
  const closeDeleting = () => {
    setDeleting(null)
    setActionError(null)
  }

  const invalidateTags = async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: TAGS_QUERY_KEY }),
      // Tag names are also rendered from these two caches: the per-frame chips
      // in the timeline detail panel and the server-supplied names on search
      // result cards. Without them a renamed tag stays stale there for up to
      // the 30s staleTime.
      queryClient.invalidateQueries({ queryKey: ['frame-tags'] }),
      queryClient.invalidateQueries({ queryKey: ['search'] }),
    ])
  }

  const onMutationError = (error: unknown) => {
    setActionError(error instanceof Error ? error.message : String(error))
  }

  const renameMutation = useMutation({
    // Draft values are passed in rather than read from the closure, so the
    // request can never carry a value from an older render.
    mutationFn: ({ tag, name, color }: { tag: Tag; name: string; color: string }) => updateTag(tag.id, { name, color }),
    onSuccess: async () => {
      closeEditing()
      await invalidateTags()
    },
    onError: onMutationError,
  })

  const deleteMutation = useMutation({
    mutationFn: (tag: Tag) => deleteTag(tag.id),
    onSuccess: async () => {
      closeDeleting()
      await invalidateTags()
    },
    onError: onMutationError,
  })

  const trimmedName = draftName.trim()
  // Exact comparison, matching SQLite: `tags.name` is UNIQUE and TEXT
  // comparison is BINARY, so "Work" and "work" are distinct rows. A
  // case-insensitive guard would reject renames the database accepts — and
  // would permanently disable Save (blocking even a colour change) for a tag
  // whose case-variant twin already exists.
  const duplicateName = tags.some((tag) => tag.id !== editing?.id && tag.name === trimmedName)
  const renameDisabled = trimmedName.length === 0 || duplicateName || renameMutation.isPending

  return (
    <Card variant="default" padding="lg">
      <CardTitle sticky>{t('settings.tagManagementTitle', { defaultValue: 'Tag management' })}</CardTitle>
      <p className={cn('mb-4 text-sm', colors.text.secondary)}>
        {t('settings.tagManagementDesc', {
          defaultValue:
            'Rename or delete tags you created on the timeline. Deleting a tag removes it from every frame.',
        })}
      </p>

      {isLoading ? (
        <div className="flex h-24 items-center justify-center">
          <Spinner />
        </div>
      ) : tags.length === 0 ? (
        <EmptyState
          icon={<TagIcon className="h-8 w-8" aria-hidden="true" />}
          title={t('settings.tagManagementEmpty', { defaultValue: 'No tags yet' })}
          description={t('settings.tagManagementEmptyDesc', {
            defaultValue: 'Tags you add to frames on the timeline show up here.',
          })}
        />
      ) : (
        <ul className="divide-y divide-border">
          {tags.map((tag) => (
            <li key={tag.id} className="flex items-center gap-3 py-2">
              <span
                className={cn(iconSize.xs, 'shrink-0 rounded-full')}
                style={{ backgroundColor: tag.color }}
                aria-hidden="true"
              />
              <span className={cn('min-w-0 flex-1 truncate text-sm', colors.text.primary)}>{tag.name}</span>
              <Button
                type="button"
                variant="secondary"
                size="sm"
                onClick={() => {
                  setDeleting(null)
                  setEditing(tag)
                }}
              >
                {t('settings.tagRename', { defaultValue: 'Rename' })}
              </Button>
              <Button
                type="button"
                variant="ghost"
                size="sm"
                onClick={() => {
                  setEditing(null)
                  setActionError(null)
                  setDeleting(tag)
                }}
              >
                {t('common.delete', { defaultValue: 'Delete' })}
              </Button>
            </li>
          ))}
        </ul>
      )}

      <Dialog open={editing !== null} onClose={closeEditing}>
        <DialogContent size="sm">
          <DialogTitle>{t('settings.tagRenameTitle', { defaultValue: 'Rename tag' })}</DialogTitle>
          <DialogBody className="space-y-4">
            <Input
              value={draftName}
              onChange={(e) => setDraftName(e.target.value)}
              aria-label={t('settings.tagNameLabel', { defaultValue: 'Tag name' })}
              autoFocus
            />
            <div className="flex flex-wrap gap-2">
              {TAG_COLORS.map((color) => (
                <button
                  key={color}
                  type="button"
                  className={cn(
                    iconSize.md,
                    `rounded-full ${motion.transform}`,
                    draftColor === color && 'scale-110 ring-2 ring-slate-400 ring-offset-1',
                  )}
                  style={{ backgroundColor: color }}
                  onClick={() => setDraftColor(color)}
                  aria-label={`${t('timeline.selectColor')} ${color}`}
                  aria-pressed={draftColor === color}
                />
              ))}
            </div>
            {duplicateName && (
              <Alert variant="warning">
                {t('settings.tagNameDuplicate', { defaultValue: 'Another tag already uses this name.' })}
              </Alert>
            )}
            {actionError && editing && <Alert variant="error">{actionError}</Alert>}
          </DialogBody>
          <DialogFooter>
            <Button type="button" variant="ghost" size="sm" onClick={closeEditing}>
              {t('common.cancel')}
            </Button>
            <Button
              type="button"
              variant="primary"
              size="sm"
              disabled={renameDisabled}
              isLoading={renameMutation.isPending}
              onClick={() => editing && renameMutation.mutate({ tag: editing, name: trimmedName, color: draftColor })}
            >
              {t('common.save')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={deleting !== null} onClose={closeDeleting}>
        <DialogContent size="sm">
          <DialogTitle>{t('settings.tagDeleteTitle', { defaultValue: 'Delete tag' })}</DialogTitle>
          <DialogBody className="space-y-4">
            <p className={cn('text-sm', colors.text.secondary)}>
              {t('settings.tagDeleteConfirm', {
                name: deleting?.name ?? '',
                defaultValue: '"{{name}}" will be removed from every frame it is attached to. This cannot be undone.',
              })}
            </p>
            {actionError && deleting && <Alert variant="error">{actionError}</Alert>}
          </DialogBody>
          <DialogFooter>
            <Button type="button" variant="ghost" size="sm" onClick={closeDeleting}>
              {t('common.cancel')}
            </Button>
            <Button
              type="button"
              variant="danger"
              size="sm"
              isLoading={deleteMutation.isPending}
              onClick={() => deleting && deleteMutation.mutate(deleting)}
            >
              {t('common.delete', { defaultValue: 'Delete' })}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </Card>
  )
}
