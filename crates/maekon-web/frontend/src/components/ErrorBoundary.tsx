import { Component, type ErrorInfo, type ReactNode } from 'react'
import { type WithTranslation, withTranslation } from 'react-i18next'
import { typography } from '../styles/tokens'

// ErrorBoundary Props: children + optional fallback + i18next HOC props
interface OwnProps {
  children: ReactNode
  fallback?: ReactNode
}

type Props = OwnProps & WithTranslation

interface State {
  hasError: boolean
  error: Error | null
}

// Utility that determines whether the error is a network/server-offline error
function isNetworkError(error: Error | null): boolean {
  if (!error) return false
  if (error instanceof TypeError && error.message.toLowerCase().includes('fetch')) return true
  const msg = error.message.toLowerCase()
  return ['failed to fetch', 'offline', 'econnrefused', 'timeout', 'network error'].some((kw) => msg.includes(kw))
}

// A class component is a hard requirement for a React Error Boundary, so i18n is wired in via the withTranslation HOC
class ErrorBoundaryBase extends Component<Props, State> {
  constructor(props: Props) {
    super(props)
    this.state = { hasError: false, error: null }
  }

  static getDerivedStateFromError(error: Error): State {
    return { hasError: true, error }
  }

  componentDidCatch(error: Error, errorInfo: ErrorInfo) {
    console.error('ErrorBoundary caught:', error, errorInfo)
  }

  render() {
    const { t, fallback, children } = this.props

    if (this.state.hasError) {
      if (fallback) return fallback

      const offline = isNetworkError(this.state.error)

      return (
        <div className="flex min-h-screen items-center justify-center bg-surface-muted">
          <div className="p-8 text-center" role="alert">
            {offline ? (
              <>
                <h1 className={`mb-4 ${typography.h1} text-semantic-warning`}>{t('errors.serverOffline')}</h1>
                <p className="mb-4 text-content-secondary">{t('errors.serverOfflineDesc')}</p>
                <button
                  type="button"
                  onClick={() => this.setState({ hasError: false, error: null })}
                  className="rounded bg-semantic-warning px-4 py-2 text-content-inverse hover:opacity-90"
                >
                  {t('errors.retryConnection')}
                </button>
              </>
            ) : (
              <>
                <h1 className={`mb-4 ${typography.h1} text-semantic-error`}>{t('errors.boundaryTitle')}</h1>
                <p className="mb-4 text-content-secondary">{this.state.error?.message}</p>
                <button
                  type="button"
                  onClick={() => this.setState({ hasError: false, error: null })}
                  className="rounded bg-brand px-4 py-2 text-content-inverse hover:bg-brand-hover"
                >
                  {t('errors.boundaryRetry')}
                </button>
              </>
            )}
          </div>
        </div>
      )
    }

    return children
  }
}

// Wrapped with the withTranslation HOC to inject the translation function t() into the class component
export default withTranslation()(ErrorBoundaryBase)
