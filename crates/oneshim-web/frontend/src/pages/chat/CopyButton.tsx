import { Check, Copy } from 'lucide-react'
import { useCallback, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { addToast } from '../../hooks/useToast'
import { motion } from '../../styles/tokens'
import { cn } from '../../utils/cn'

export function CopyButton({ text }: { text: string }) {
  const { t } = useTranslation()
  const [copied, setCopied] = useState(false)
  const handleCopy = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(text)
      setCopied(true)
      setTimeout(() => setCopied(false), 2000)
    } catch (e) {
      console.warn('clipboard.writeText failed:', e)
      addToast('error', t('chat.copy_failed', 'Failed to copy the content.'), 4000)
    }
  }, [text, t])
  return (
    <button
      type="button"
      onClick={handleCopy}
      className={cn(
        'absolute top-2 right-2 rounded bg-surface-elevated/40 p-1.5 text-content-inverse/60 opacity-0 hover:bg-surface-elevated/60 group-hover:opacity-100',
        motion.opacity,
      )}
      title={t('chat.copy')}
    >
      {copied ? <Check size={14} /> : <Copy size={14} />}
    </button>
  )
}
