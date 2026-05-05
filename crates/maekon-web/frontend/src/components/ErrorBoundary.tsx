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

// 네트워크/서버 오프라인 에러 여부 판단 유틸
function isNetworkError(error: Error | null): boolean {
  if (!error) return false
  if (error instanceof TypeError && error.message.toLowerCase().includes('fetch')) return true
  const msg = error.message.toLowerCase()
  return ['failed to fetch', 'offline', 'econnrefused', 'timeout', 'network error'].some((kw) => msg.includes(kw))
}

// 클래스 컴포넌트는 React Error Boundary 필수 조건이므로 withTranslation HOC로 i18n 연동
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

// withTranslation HOC로 감싸 번역 함수 t()를 class 컴포넌트에 주입
export default withTranslation()(ErrorBoundaryBase)
