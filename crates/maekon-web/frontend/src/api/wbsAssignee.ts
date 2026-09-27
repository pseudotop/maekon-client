import type {
  WbsAuthorization,
  WbsCancellation,
  WbsCandidateChoice,
  WbsConsentOffer,
  WbsExecution,
  WbsRecommendation,
  WbsSessionView,
  WbsTicket,
} from '../overlay/wbsTypes'

async function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke: tauriInvoke } = await import('@tauri-apps/api/core')
  return tauriInvoke<T>(command, args)
}

export const offerWbsDocumentConsent = () => invoke<WbsConsentOffer>('wbs_offer_document_consent')
export const openWbsSession = (
  acceptance: Pick<WbsConsentOffer, 'offer_id' | 'nonce' | 'scope_id' | 'notice_version'>,
) => invoke<WbsSessionView>('wbs_open_session', { acceptance })
export const recommendWbsAssignees = (auth: WbsAuthorization) => invoke<WbsRecommendation>('wbs_recommend', { auth })
export const confirmWbsCandidate = (auth: WbsAuthorization, choice: WbsCandidateChoice) =>
  invoke<WbsTicket>('wbs_confirm_candidate', { auth, choice })
export const executeWbsAssignee = (auth: WbsAuthorization, ticket: WbsTicket) =>
  invoke<WbsExecution>('wbs_execute', { auth, ticket })
export const readWbsExecution = (auth: WbsAuthorization, operation_id: string) =>
  invoke<WbsExecution>('wbs_read_execution', { auth, operation: { operation_id } })
export const cancelWbsSession = (auth: WbsAuthorization) => invoke<WbsCancellation>('wbs_cancel_session', { auth })
