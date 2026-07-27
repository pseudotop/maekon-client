import { useCallback } from 'react'
import { useCaptureReauthRecovery } from '../components/CaptureReauthGate'
import { addToast } from './useToast'

/**
 * Shared failure path for capture-history mutations.
 *
 * An idle-expired re-auth session opens the gate's action-scoped prompt and
 * retries the exact local mutation once authentication succeeds. All other
 * failures, including a failed retry, are surfaced to the user.
 */
export function useCaptureMutationRecovery(errorMessage: string) {
  const { requestCaptureReauth } = useCaptureReauthRecovery()

  return useCallback(
    async (error: unknown, retry: () => void | Promise<void>) => {
      const retryWithVisibleFailure = async () => {
        try {
          await retry()
        } catch {
          addToast('error', errorMessage)
        }
      }

      try {
        const handled = await requestCaptureReauth(error, retryWithVisibleFailure)
        if (!handled) addToast('error', errorMessage)
      } catch {
        addToast('error', errorMessage)
      }
    },
    [errorMessage, requestCaptureReauth],
  )
}
