import { useCallback, useEffect, useRef, useState } from 'react'
import { isIpcError } from '../../api/desktop'
import {
  cancelWbsSession,
  confirmWbsCandidate,
  executeWbsAssignee,
  offerWbsDocumentConsent,
  openWbsSession,
  readWbsExecution,
  recommendWbsAssignees,
} from '../../api/wbsAssignee'
import type { WbsCandidate, WbsConsentOffer, WbsExecution, WbsRecommendation, WbsSessionView } from '../wbsTypes'

export type WbsStatus =
  | 'idle'
  | 'offering'
  | 'consent'
  | 'loading'
  | 'ready'
  | 'executing'
  | 'unavailable'
  | 'cancelled'

const isFuture = (value: string) => Number.isFinite(Date.parse(value)) && Date.parse(value) > Date.now()

const preExecutionCodes = new Set([
  'wbs_automation_disabled',
  'wbs_consent_required',
  'wbs_consent_changed',
  'wbs_policy_blocked',
  'wbs_sandbox_unsupported',
  'wbs_configuration_changed',
  'wbs_expired',
])
// Only fixed codes received before dispatch can explain why a write was not requested.
const preExecutionError = (value: unknown) =>
  isIpcError(value) && preExecutionCodes.has(value.code) ? value.code : 'wbs_unavailable'

async function disposeSession(value: WbsSessionView | null) {
  if (!value) return
  try {
    await cancelWbsSession(value.authorization)
  } catch {
    // Cleanup must also tolerate a closed IPC bridge after unmount.
  }
}

export function useWbsAssignee() {
  const [status, setStatus] = useState<WbsStatus>('idle')
  const [offer, setOffer] = useState<WbsConsentOffer | null>(null)
  const [recommendation, setRecommendation] = useState<WbsRecommendation | null>(null)
  const [execution, setExecution] = useState<WbsExecution | null>(null)
  const [error, setError] = useState<string | null>(null)
  const session = useRef<WbsSessionView | null>(null)
  const currentOffer = useRef<WbsConsentOffer | null>(null)
  const currentRecommendation = useRef<WbsRecommendation | null>(null)
  const operationId = useRef<string | null>(null)
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const epoch = useRef(0)
  const busy = useRef(false)
  const mounted = useRef(true)
  const invalidate = useCallback(() => ++epoch.current, [])
  const live = useCallback(
    (value: number, expected?: WbsSessionView) =>
      mounted.current && value === epoch.current && (!expected || session.current === expected),
    [],
  )
  const clearTimer = useCallback(() => {
    if (timer.current !== null) clearTimeout(timer.current)
    timer.current = null
  }, [])
  const replaceOffer = useCallback((value: WbsConsentOffer | null) => {
    currentOffer.current = value
    setOffer(value)
  }, [])
  const replaceRecommendation = useCallback((value: WbsRecommendation | null) => {
    currentRecommendation.current = value
    setRecommendation(value)
  }, [])
  const clearOperation = useCallback(() => {
    operationId.current = null
    setExecution(null)
  }, [])

  const cancel = useCallback(async () => {
    invalidate()
    clearTimer()
    const current = session.current
    session.current = null
    currentOffer.current = null
    currentRecommendation.current = null
    operationId.current = null
    busy.current = false
    if (mounted.current) {
      setOffer(null)
      setRecommendation(null)
      setExecution(null)
      setError(null)
      setStatus('cancelled')
    }
    void disposeSession(current)
  }, [clearTimer, invalidate])

  useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
      invalidate()
      clearTimer()
      const current = session.current
      session.current = null
      currentOffer.current = null
      currentRecommendation.current = null
      operationId.current = null
      busy.current = false
      void disposeSession(current)
    }
  }, [clearTimer, invalidate])

  const armExpiry = useCallback(
    (current: WbsSessionView, expiresAt = current.expires_at) => {
      clearTimer()
      timer.current = setTimeout(
        () => {
          if (mounted.current && session.current === current) {
            void cancel()
            setError('wbs_expired')
          }
        },
        Math.min(Date.parse(current.expires_at), Date.parse(expiresAt)) - Date.now(),
      )
    },
    [cancel, clearTimer],
  )

  const publishRecommendation = useCallback(
    (next: WbsRecommendation, current: WbsSessionView) => {
      const expired = !isFuture(current.expires_at) || !isFuture(next.expires_at)
      if (expired || next.session_id !== current.authorization.session_id) {
        replaceRecommendation(null)
        setStatus('unavailable')
        setError(expired ? 'wbs_expired' : 'wbs_unavailable')
        return
      }
      replaceRecommendation(next)
      armExpiry(current, next.expires_at)
      setStatus('ready')
    },
    [armExpiry, replaceRecommendation],
  )

  const requestConsent = useCallback(async () => {
    if (!mounted.current || busy.current) return
    busy.current = true
    const token = invalidate()
    clearTimer()
    const previous = session.current
    session.current = null
    void disposeSession(previous)
    clearOperation()
    replaceOffer(null)
    replaceRecommendation(null)
    setStatus('offering')
    setError(null)
    try {
      const next = await offerWbsDocumentConsent()
      if (!live(token)) return
      if (!isFuture(next.expires_at)) {
        setStatus('unavailable')
        setError('wbs_expired')
        return
      }
      replaceOffer(next)
      setStatus('consent')
    } catch (caught) {
      if (live(token)) {
        setStatus('unavailable')
        setError(preExecutionError(caught))
      }
    } finally {
      if (live(token)) busy.current = false
    }
  }, [clearOperation, clearTimer, invalidate, live, replaceOffer, replaceRecommendation])

  const acceptConsent = useCallback(async () => {
    const offered = currentOffer.current
    if (!mounted.current || !offered || busy.current) return
    if (!isFuture(offered.expires_at)) {
      replaceOffer(null)
      setStatus('unavailable')
      setError('wbs_expired')
      return
    }
    busy.current = true
    const token = invalidate()
    replaceOffer(null)
    replaceRecommendation(null)
    clearOperation()
    setStatus('loading')
    setError(null)
    try {
      const opened = await openWbsSession({
        offer_id: offered.offer_id,
        nonce: offered.nonce,
        scope_id: offered.scope_id,
        notice_version: offered.notice_version,
      })
      if (!live(token)) return void disposeSession(opened)
      if (!isFuture(opened.expires_at) || opened.scope_id !== offered.scope_id) {
        setStatus('unavailable')
        setError(!isFuture(opened.expires_at) ? 'wbs_expired' : 'wbs_unavailable')
        return void disposeSession(opened)
      }
      session.current = opened
      armExpiry(opened)
      const next = await recommendWbsAssignees(opened.authorization)
      if (live(token, opened)) publishRecommendation(next, opened)
    } catch (caught) {
      if (live(token)) {
        setStatus('unavailable')
        setError(preExecutionError(caught))
      }
    } finally {
      if (live(token)) busy.current = false
    }
  }, [armExpiry, clearOperation, invalidate, live, publishRecommendation, replaceOffer, replaceRecommendation])

  const refresh = useCallback(async () => {
    const current = session.current
    if (!mounted.current || !current || busy.current || !isFuture(current.expires_at)) return
    busy.current = true
    const token = invalidate()
    clearOperation()
    replaceRecommendation(null)
    armExpiry(current)
    setStatus('loading')
    setError(null)
    try {
      const next = await recommendWbsAssignees(current.authorization)
      if (live(token, current)) publishRecommendation(next, current)
    } catch (caught) {
      if (live(token, current)) {
        setStatus('unavailable')
        setError(preExecutionError(caught))
      }
    } finally {
      if (live(token, current)) busy.current = false
    }
  }, [armExpiry, clearOperation, invalidate, live, publishRecommendation, replaceRecommendation])

  const choose = useCallback(
    async (candidate: WbsCandidate) => {
      const current = session.current
      const view = currentRecommendation.current
      const selected = view?.candidates.find(
        (value) => value.candidate_id === candidate.candidate_id && value.candidate_proof === candidate.candidate_proof,
      )
      if (
        !mounted.current ||
        !current ||
        !view ||
        !selected ||
        selected !== candidate ||
        !selected.eligible ||
        busy.current ||
        operationId.current !== null
      )
        return
      if (!isFuture(current.expires_at) || !isFuture(view.expires_at)) {
        replaceRecommendation(null)
        setStatus('unavailable')
        setError('wbs_expired')
        return
      }
      busy.current = true
      const token = invalidate()
      clearOperation()
      let attemptedId: string | null = null
      setStatus('executing')
      setError(null)
      try {
        const ticket = await confirmWbsCandidate(current.authorization, {
          query_generation: view.query_generation,
          candidate_id: selected.candidate_id,
          candidate_proof: selected.candidate_proof,
        })
        if (!live(token, current)) return
        const expired = !isFuture(ticket.expires_at) || !isFuture(current.expires_at) || !isFuture(view.expires_at)
        if (expired || ticket.session_id !== current.authorization.session_id || !ticket.operation_id) {
          replaceRecommendation(null)
          setStatus('unavailable')
          setError(expired ? 'wbs_expired' : 'wbs_unavailable')
          return
        }
        // Only a dispatched, identified operation can have an unknown write outcome.
        attemptedId = ticket.operation_id
        operationId.current = attemptedId
        const result = await executeWbsAssignee(current.authorization, ticket)
        if (!live(token, current)) return
        if (result.operation_id !== attemptedId) throw new Error('wbs_operation_mismatch')
        try {
          const readback = await readWbsExecution(current.authorization, attemptedId)
          if (!live(token, current)) return
          if (readback.operation_id !== attemptedId) throw new Error('wbs_operation_mismatch')
          setExecution(readback)
          setStatus('ready')
        } catch {
          if (live(token, current)) {
            setExecution(result)
            setStatus('ready')
            setError('wbs_readback_unavailable')
          }
        }
      } catch (caught) {
        if (live(token, current)) {
          if (attemptedId === null) {
            replaceRecommendation(null)
            setStatus('unavailable')
            setError(preExecutionError(caught))
          } else {
            setExecution({
              operation_id: attemptedId,
              disposition: 'outcome_unknown',
              native_outcome: 'outcome_unknown',
              readback: 'not_read',
              audit: 'not_started',
            })
            setStatus('ready')
            setError('wbs_outcome_unknown')
          }
        }
      } finally {
        if (live(token, current)) busy.current = false
      }
    },
    [clearOperation, invalidate, live, replaceRecommendation],
  )

  const checkExecution = useCallback(async () => {
    const current = session.current
    const id = operationId.current
    if (!mounted.current || !current || !id || busy.current) return
    busy.current = true
    const token = invalidate()
    setError(null)
    try {
      const result = await readWbsExecution(current.authorization, id)
      if (!live(token, current)) return
      if (result.operation_id !== id) throw new Error('wbs_operation_mismatch')
      setExecution(result)
      setStatus('ready')
    } catch {
      if (live(token, current)) setError('wbs_readback_unavailable')
    } finally {
      if (live(token, current)) busy.current = false
    }
  }, [invalidate, live])

  return {
    status,
    offer,
    recommendation,
    execution,
    error,
    requestConsent,
    acceptConsent,
    refresh,
    choose,
    cancel,
    checkExecution,
  }
}
