import { useTranslation } from 'react-i18next'
import { type ExtensionView, useExtensions } from '../hooks/useExtensions'

/**
 * Extension registry panel (ADR-029 §5, #8586).
 *
 * Renders the eight readiness axes. A single summary label is shown only when
 * the backend derived one; otherwise the axes stand alone. An unavailable,
 * revoked, stale, or partially configured extension is never labelled as
 * installed or connected.
 *
 * There is no marketplace, rating, or install-count surface here — the registry
 * backend is local-only, and a storefront UI would be theater (#8586 AC).
 */

export function ExtensionRegistryPanel() {
  const { t } = useTranslation()
  const { extensions, loading, error, install, setEnablement, rollback } = useExtensions()

  if (loading) {
    return (
      <div className="extension-registry" aria-busy="true">
        {t('extensions.loading', 'Loading extensions…')}
      </div>
    )
  }

  const axisLabel = (key: string, fallback: string, value: string) =>
    `${t(key, fallback)}: ${t(`extensions.axisValue.${value}`, value)}`

  const renderRow = (ext: ExtensionView) => {
    const unavailable = ext.availability.state === 'unavailable'
    return (
      <li key={ext.install_id} data-testid={`extension-${ext.install_id}`}>
        <span className="extension-registry__id">{ext.extension_id}</span>
        <span className="extension-registry__version">{ext.version}</span>

        {ext.summary_label ? (
          <span className="extension-registry__summary" data-testid={`summary-${ext.install_id}`}>
            {t(`extensions.summary.${ext.summary_label}`, ext.summary_label)}
          </span>
        ) : (
          <span className="extension-registry__summary extension-registry__summary--none">
            {t('extensions.summaryUnavailable', 'In transition — see details')}
          </span>
        )}

        {/* The axes are always rendered, so a partial state can never hide. */}
        <ul className="extension-registry__axes">
          <li>{axisLabel('extensions.axis.installation', 'Installation', ext.installation)}</li>
          <li>{axisLabel('extensions.axis.enablement', 'Enablement', ext.enablement)}</li>
          <li>{axisLabel('extensions.axis.authentication', 'Account', ext.authentication)}</li>
          <li>{axisLabel('extensions.axis.grant', 'Capability grant', ext.grant)}</li>
          <li>{axisLabel('extensions.axis.update', 'Update', ext.update)}</li>
          <li>{axisLabel('extensions.axis.health', 'Health', ext.health.state)}</li>
          {unavailable && (
            <li className="extension-registry__unavailable" role="status">
              {t('extensions.unavailableReason', 'Unavailable: {{reason}}', {
                reason: ext.availability.reason ?? 'unknown',
              })}
            </li>
          )}
        </ul>

        {ext.installation === 'not_installed' && !unavailable && (
          <button type="button" onClick={() => void install(ext.install_id, ext.revision)}>
            {t('extensions.install', 'Install')}
          </button>
        )}
        {ext.installation === 'installed' && (
          <button
            type="button"
            onClick={() => void setEnablement(ext.install_id, ext.enablement === 'disabled', ext.revision)}
          >
            {ext.enablement === 'disabled' ? t('extensions.enable', 'Enable') : t('extensions.disable', 'Disable')}
          </button>
        )}
        {ext.previous_version && (
          <button type="button" onClick={() => void rollback(ext.install_id, ext.revision)}>
            {t('extensions.rollback', 'Roll back to {{version}}', {
              version: ext.previous_version,
            })}
          </button>
        )}
      </li>
    )
  }

  return (
    <div className="extension-registry">
      {error && (
        <p role="alert" className="extension-registry__error">
          {t('extensions.loadError', 'Could not load extensions: {{error}}', { error })}
        </p>
      )}
      <h2>{t('extensions.heading', 'Extensions')}</h2>
      {extensions.length === 0 ? (
        <p className="extension-registry__empty">{t('extensions.empty', 'No extensions are registered.')}</p>
      ) : (
        <ul>{extensions.map(renderRow)}</ul>
      )}
    </div>
  )
}
