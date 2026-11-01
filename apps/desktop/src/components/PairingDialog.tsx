import { useEffect, useState } from 'react';
import { api } from '../api/client';
import { useApp } from '../state/appStore';
import { Icon } from './Icon';
import { Modal } from './Modal';

function formatSas(sas: string | null): string {
  if (!sas) {
    return '••• •••';
  }
  return sas.replace(/\s+/g, '').replace(/(.{3})/g, '$1 ').trim();
}

export function PairingDialog() {
  const { state, t, reportError, closePairing } = useApp();
  const pairing = state.pairing;
  const stage = pairing?.stage ?? null;
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    setBusy(false);
  }, [stage]);

  if (!pairing || !stage) {
    return null;
  }

  const pending = stage === 'sas_required';
  const deviceName = pairing.device_name ?? t('pairing.unknownDevice');

  const handleClose = async () => {
    if (!pending) {
      closePairing();
      return;
    }
    if (busy) {
      return;
    }
    setBusy(true);
    try {
      await api.cancelPairing();
    } catch (error) {
      reportError(error, t('pairing.cancelFailed'));
    } finally {
      setBusy(false);
    }
  };

  const handleConfirm = async (accept: boolean) => {
    setBusy(true);
    try {
      await api.confirmPairing(accept);
    } catch (error) {
      reportError(error, t('pairing.confirmFailed'));
    } finally {
      setBusy(false);
    }
  };

  const renderBody = () => {
    switch (stage) {
      case 'sas_required':
        return (
          <>
            <p className="pairing-lead">{t('pairing.sasHelp')}</p>
            <div className="pairing-code-block sas">
              <span className="eyebrow">{t('pairing.sasLabel')}</span>
              <strong className="pairing-code" aria-label={t('pairing.sasAria')}>
                {formatSas(pairing.sas)}
              </strong>
            </div>
            {pairing.message ? <p className="pairing-note">{pairing.message}</p> : null}
          </>
        );
      case 'success':
        return (
          <div className="pairing-result is-success">
            <span className="result-icon" aria-hidden="true">
              <Icon name="check" size={28} />
            </span>
            <strong>{t('pairing.successTitle')}</strong>
            <span>{t('pairing.successBody', { name: deviceName })}</span>
          </div>
        );
      case 'failed':
        return (
          <div className="pairing-result is-failure">
            <span className="result-icon" aria-hidden="true">
              <Icon name="warning" size={28} />
            </span>
            <strong>{t('pairing.failedTitle')}</strong>
            <span>{pairing.message ?? t('pairing.failedFallback')}</span>
          </div>
        );
      case 'cancelled':
        return (
          <div className="pairing-result is-cancelled">
            <span className="result-icon" aria-hidden="true">
              <Icon name="info" size={28} />
            </span>
            <strong>{t('pairing.cancelledTitle')}</strong>
            <span>{pairing.message ?? t('pairing.cancelledBody')}</span>
          </div>
        );
      default:
        return null;
    }
  };

  const footer = pending ? (
    <>
      <button
        type="button"
        className="btn ghost-danger"
        disabled={busy}
        onClick={() => void handleConfirm(false)}
      >
        {t('pairing.reject')}
      </button>
      <button
        type="button"
        className="btn primary"
        disabled={busy}
        onClick={() => void handleConfirm(true)}
      >
        {busy ? t('common.busy') : t('pairing.confirm')}
      </button>
    </>
  ) : (
    <button type="button" className="btn primary" onClick={closePairing}>
      {t('common.close')}
    </button>
  );

  return (
    <Modal
      open
      title={pending ? t('pairing.sasTitle') : t('pairing.title')}
      onClose={() => void handleClose()}
      dismissible
      size="sm"
      footer={footer}
    >
      <div className="pairing-device">
        <span className="pairing-device-icon" aria-hidden="true">
          <Icon name="link" size={16} />
        </span>
        <div>
          <span>{t('pairing.device')}</span>
          <strong>{deviceName}</strong>
        </div>
        <span className="pairing-stage-chip">{pending ? 'SAS' : 'LAN'}</span>
      </div>
      {renderBody()}
    </Modal>
  );
}
