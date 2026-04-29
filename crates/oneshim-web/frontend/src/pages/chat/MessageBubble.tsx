import { AlertTriangle, Bot, Loader2, RefreshCw, User, Wrench } from 'lucide-react'
import React, { useMemo } from 'react'
import { useTranslation } from 'react-i18next'
import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import { Button, Card, CardContent } from '../../components/ui'
import { iconSize, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import { CopyButton } from './CopyButton'
import { highlightText } from './highlightText'
import type { ChatMessage } from './types'

// Lazy-loaded syntax highlighter — only fetched when a fenced code block is rendered
const LazySyntaxHighlighter = React.lazy(async () => {
  const [
    { default: SyntaxHighlighter },
    { oneDark },
    javascript,
    typescript,
    python,
    bash,
    jsonLang,
    cssLang,
    rust,
    sql,
    yaml,
    markdownLang,
  ] = await Promise.all([
    import('react-syntax-highlighter/dist/esm/prism-light'),
    import('react-syntax-highlighter/dist/esm/styles/prism'),
    import('react-syntax-highlighter/dist/esm/languages/prism/javascript'),
    import('react-syntax-highlighter/dist/esm/languages/prism/typescript'),
    import('react-syntax-highlighter/dist/esm/languages/prism/python'),
    import('react-syntax-highlighter/dist/esm/languages/prism/bash'),
    import('react-syntax-highlighter/dist/esm/languages/prism/json'),
    import('react-syntax-highlighter/dist/esm/languages/prism/css'),
    import('react-syntax-highlighter/dist/esm/languages/prism/rust'),
    import('react-syntax-highlighter/dist/esm/languages/prism/sql'),
    import('react-syntax-highlighter/dist/esm/languages/prism/yaml'),
    import('react-syntax-highlighter/dist/esm/languages/prism/markdown'),
  ])

  for (const [name, lang] of [
    ['javascript', javascript.default],
    ['js', javascript.default],
    ['jsx', javascript.default],
    ['typescript', typescript.default],
    ['ts', typescript.default],
    ['tsx', typescript.default],
    ['python', python.default],
    ['py', python.default],
    ['bash', bash.default],
    ['sh', bash.default],
    ['shell', bash.default],
    ['json', jsonLang.default],
    ['css', cssLang.default],
    ['rust', rust.default],
    ['rs', rust.default],
    ['sql', sql.default],
    ['yaml', yaml.default],
    ['yml', yaml.default],
    ['markdown', markdownLang.default],
    ['md', markdownLang.default],
  ] as const) {
    SyntaxHighlighter.registerLanguage(name, lang)
  }

  function LazyHighlighterWrapper(props: { language: string; children: string }) {
    return (
      <SyntaxHighlighter
        style={oneDark}
        language={props.language}
        PreTag="div"
        customStyle={{ margin: 0, borderRadius: '0.375rem', fontSize: '0.8rem' }}
      >
        {props.children}
      </SyntaxHighlighter>
    )
  }
  return { default: LazyHighlighterWrapper }
})

export function MessageBubble({
  msg,
  onRetry,
  highlight,
}: {
  msg: ChatMessage
  onRetry: () => void
  highlight?: string
}) {
  const { t } = useTranslation()

  // Memoize markdown components to capture msg.streaming in closure
  // Must be called before any early returns (rules of hooks)
  const mdComponents = useMemo(
    () => ({
      code({ className, children, ...props }: { className?: string; children?: React.ReactNode }) {
        const match = /language-(\w+)/.exec(className || '')
        const code = String(children).replace(/\n$/, '')

        // Fenced code block with language
        if (match) {
          // During streaming: plain pre (avoid repeated highlighting runs)
          if (msg.streaming) {
            return (
              <pre className={cn('my-2 overflow-x-auto rounded bg-surface-sunken p-3 text-xs', typography.family.mono)}>
                {code}
              </pre>
            )
          }
          // After done: full syntax highlighting (lazy-loaded)
          return (
            <div className="group relative my-2">
              <CopyButton text={code} />
              <React.Suspense
                fallback={
                  <pre className={cn('overflow-x-auto rounded bg-surface-sunken p-3 text-xs', typography.family.mono)}>
                    {code}
                  </pre>
                }
              >
                <LazySyntaxHighlighter language={match[1]}>{code}</LazySyntaxHighlighter>
              </React.Suspense>
            </div>
          )
        }

        // Inline code
        return (
          <code className={cn('rounded bg-surface-sunken px-1 py-0.5 text-xs', typography.family.mono)} {...props}>
            {children}
          </code>
        )
      },
      p: ({ children }: { children?: React.ReactNode }) => <p className="mb-2 last:mb-0">{children}</p>,
      ul: ({ children }: { children?: React.ReactNode }) => <ul className="mb-2 ml-4 list-disc">{children}</ul>,
      ol: ({ children }: { children?: React.ReactNode }) => <ol className="mb-2 ml-4 list-decimal">{children}</ol>,
      li: ({ children }: { children?: React.ReactNode }) => <li className="mb-0.5">{children}</li>,
      h3: ({ children }: { children?: React.ReactNode }) => (
        <h3 className={cn('mt-2 mb-1', typography.weight.semibold)}>{children}</h3>
      ),
      a: ({ href, children }: { href?: string; children?: React.ReactNode }) => (
        <a href={href} target="_blank" rel="noopener noreferrer" className="text-brand-text underline">
          {children}
        </a>
      ),
      blockquote: ({ children }: { children?: React.ReactNode }) => (
        <blockquote className="border-brand/30 border-l-2 pl-3 text-content-secondary italic">{children}</blockquote>
      ),
      table: ({ children }: { children?: React.ReactNode }) => (
        <div className="overflow-x-auto">
          <table className="border-collapse text-xs">{children}</table>
        </div>
      ),
      th: ({ children }: { children?: React.ReactNode }) => (
        <th className={cn('border border-muted px-2 py-1', typography.weight.medium)}>{children}</th>
      ),
      td: ({ children }: { children?: React.ReactNode }) => (
        <td className="border border-muted px-2 py-1">{children}</td>
      ),
    }),
    [msg.streaming],
  )

  if (msg.error) {
    return (
      <Card variant="default" padding="sm" className="border-semantic-error/30 bg-semantic-error/5">
        <CardContent>
          <div className="flex items-start gap-2">
            <AlertTriangle className={cn(iconSize.base, 'mt-0.5 shrink-0 text-semantic-error')} />
            <div className="min-w-0 flex-1">
              <p className={cn('text-semantic-error text-xs', typography.weight.medium)}>{msg.error.code}</p>
              <p className="mt-0.5 text-content-secondary text-xs">{msg.error.message}</p>
              {msg.error.retryable && (
                <Button variant="ghost" size="sm" onClick={onRetry} className="mt-1 text-xs">
                  <RefreshCw className={iconSize.xs} /> {t('chat.retry')}
                </Button>
              )}
            </div>
          </div>
        </CardContent>
      </Card>
    )
  }

  if (msg.tool_use) {
    const statusCls =
      msg.tool_use.status === 'completed'
        ? 'bg-semantic-success/20 text-semantic-success'
        : msg.tool_use.status === 'failed'
          ? 'bg-semantic-error/20 text-semantic-error'
          : 'bg-surface-elevated text-content-secondary'
    return (
      <Card variant="default" padding="sm" className="border-border/50">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2">
            {msg.tool_use.status === 'started' ? (
              <Loader2 className={cn(iconSize.xs, 'animate-spin text-content-secondary')} />
            ) : (
              <Wrench className={iconSize.xs} />
            )}
            <span className={cn('text-xs', typography.weight.medium)}>{msg.tool_use.tool}</span>
          </div>
          <span className={cn('rounded px-1.5 py-0.5 text-[10px]', statusCls)}>{msg.tool_use.status}</span>
        </div>
        {msg.tool_use.input && (
          <details className="mt-1">
            <summary className="cursor-pointer text-content-secondary text-xs">Input</summary>
            <pre className="mt-1 overflow-x-auto rounded bg-surface-sunken p-2 text-[10px]">
              {JSON.stringify(msg.tool_use.input, null, 2)}
            </pre>
          </details>
        )}
        {msg.tool_use.result && (
          <details open className="mt-1">
            <summary className="cursor-pointer text-content-secondary text-xs">Result</summary>
            <pre className="mt-1 overflow-x-auto whitespace-pre-wrap rounded bg-surface-sunken p-2 text-[10px]">
              {msg.tool_use.result}
            </pre>
          </details>
        )}
      </Card>
    )
  }

  if (msg.tool_call_delta) {
    return (
      <Card variant="default" padding="sm" className="border-border/50 bg-surface-base">
        <CardContent>
          <div className="flex items-start gap-2">
            <Loader2 className={cn(iconSize.xs, 'mt-0.5 shrink-0 animate-spin text-content-secondary')} />
            <div className="min-w-0 flex-1">
              <p className={cn('text-xs', typography.weight.medium)}>{msg.tool_call_delta.name}</p>
              <pre className="mt-1 overflow-x-auto whitespace-pre-wrap rounded bg-surface-sunken p-2 text-[10px]">
                {msg.tool_call_delta.arguments}
              </pre>
            </div>
          </div>
        </CardContent>
      </Card>
    )
  }

  if (msg.thinking) {
    return (
      <Card variant="default" padding="sm" className="border-brand/20 bg-brand/5">
        <CardContent>
          <div className="flex items-start gap-2">
            <Loader2
              className={cn(iconSize.xs, !msg.thinking.done && 'animate-spin', 'mt-0.5 shrink-0 text-brand-text')}
            />
            <div className="min-w-0 flex-1">
              <p
                className={cn(
                  'text-[10px] text-content-secondary uppercase tracking-[0.14em]',
                  typography.weight.medium,
                )}
              >
                {t('chat.thinking')}
              </p>
              <p className="mt-1 whitespace-pre-wrap break-words text-content-secondary text-xs">
                {msg.thinking.content}
                {!msg.thinking.done ? '\u258C' : ''}
              </p>
            </div>
          </div>
        </CardContent>
      </Card>
    )
  }

  const isUser = msg.role === 'user'

  return (
    <div className={cn('flex gap-2', isUser ? 'justify-end' : 'justify-start')}>
      {!isUser && (
        <div className="flex h-6 w-6 shrink-0 items-center justify-center rounded-full bg-brand/10">
          <Bot className="h-3.5 w-3.5 text-brand-text" />
        </div>
      )}
      <div
        className={cn(
          'max-w-[75%] rounded-lg px-3 py-2 text-sm',
          isUser ? 'bg-brand text-content-inverse' : 'bg-surface-elevated text-content',
        )}
      >
        {isUser ? (
          <p className="whitespace-pre-wrap break-words">
            {highlight ? highlightText(msg.content, highlight) : msg.content}
          </p>
        ) : (
          <div className="prose-sm">
            <ReactMarkdown remarkPlugins={[remarkGfm]} components={mdComponents}>
              {msg.content + (msg.streaming ? '\u258C' : '')}
            </ReactMarkdown>
          </div>
        )}
        {msg.usage && (
          <p className={cn('mt-1 text-[10px]', isUser ? 'text-content-inverse/60' : 'text-content-secondary')}>
            {msg.usage.input_tokens} in / {msg.usage.output_tokens} out
          </p>
        )}
      </div>
      {isUser && (
        <div className="flex h-6 w-6 shrink-0 items-center justify-center rounded-full bg-surface-elevated">
          <User className="h-3.5 w-3.5 text-content-secondary" />
        </div>
      )}
    </div>
  )
}
