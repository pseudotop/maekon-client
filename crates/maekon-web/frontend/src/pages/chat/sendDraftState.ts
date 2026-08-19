import type { ChatMessage } from './types'

export function clearAcceptedText(current: string, submitted: string): string {
  return current === submitted ? '' : current
}

export function clearAcceptedAttachments<T>(current: T[], submitted: T[]): T[] {
  return current === submitted ? [] : current
}

export function removeOptimisticMessage(current: ChatMessage[], optimisticMessage: ChatMessage): ChatMessage[] {
  return current.filter((message) => message !== optimisticMessage)
}
