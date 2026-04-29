import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'
import { createOverride, deleteOverride, listOverrides, triggerRecluster } from '../api/client'
import type { CreateOverrideRequest, RegimeOverride } from '../api/contracts'
import { addToast } from './useToast'

export function useOverrides(from?: string, to?: string) {
  return useQuery<RegimeOverride[]>({
    queryKey: ['overrides', from, to],
    queryFn: () => listOverrides({ from, to }),
  })
}

export function useCreateOverride() {
  const queryClient = useQueryClient()
  const { t } = useTranslation()

  return useMutation({
    mutationFn: (req: CreateOverrideRequest) => createOverride(req),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['overrides'] })
      addToast('success', t('recalibration.overrideCreated'))
    },
    onError: (err: Error) => {
      addToast('error', err.message)
    },
  })
}

export function useDeleteOverride() {
  const queryClient = useQueryClient()
  const { t } = useTranslation()

  return useMutation({
    mutationFn: (id: string) => deleteOverride(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['overrides'] })
      addToast('success', t('recalibration.overrideDeleted'))
    },
    onError: (err: Error) => {
      addToast('error', err.message)
    },
  })
}

export function useRecluster() {
  const { t } = useTranslation()

  return useMutation({
    mutationFn: () => triggerRecluster(),
    onSuccess: () => {
      addToast('success', t('recalibration.reclusterSuccess'))
    },
    onError: () => {
      addToast('error', t('recalibration.reclusterError'))
    },
  })
}
