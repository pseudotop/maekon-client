/**
 * Privacy consent section — delete all data (danger zone).
 */

import { useTranslation } from 'react-i18next'
import { Button, Card, CardTitle } from '../../components/ui'
import { useTypedOutletContext } from '../../routes'
import type { PrivacyContext } from './PrivacyLayout'

export default function ConsentSection() {
  const { t } = useTranslation()
  const { deleteAllMutation, setShowDeleteAllModal } = useTypedOutletContext<PrivacyContext>('Privacy')

  return (
    <Card id="section-consent" variant="danger" padding="lg">
      <CardTitle className="mb-2 text-semantic-error">{t('privacy.deleteAllTitle')}</CardTitle>
      <p className="mb-4 text-content-secondary text-sm">{t('privacy.deleteAllDesc')}</p>
      <Button
        data-testid="delete-all"
        variant="danger"
        onClick={() => setShowDeleteAllModal(true)}
        isLoading={deleteAllMutation.isPending}
      >
        {deleteAllMutation.isPending ? t('privacy.deleting') : t('privacy.deleteAllButton')}
      </Button>
    </Card>
  )
}
