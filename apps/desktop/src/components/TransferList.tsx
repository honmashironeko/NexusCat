import type { TransferState, TransferView } from '../api/types';
import { formatBytes } from '../domain/format';
import { useApp } from '../state/appStore';
import { Icon } from './Icon';

function transferStateKey(
  state: TransferState,
):
  | 'files.state.offered'
  | 'files.state.transferring'
  | 'files.state.verifying'
  | 'files.state.done'
  | 'files.state.failed'
  | 'files.state.cancelled' {
  switch (state) {
    case 'offered':
      return 'files.state.offered';
    case 'transferring':
      return 'files.state.transferring';
    case 'verifying':
      return 'files.state.verifying';
    case 'done':
      return 'files.state.done';
    case 'failed':
      return 'files.state.failed';
    default:
      return 'files.state.cancelled';
  }
}

export function TransferList({
  transfers,
  cancellingId,
  onCancel,
}: {
  transfers: TransferView[];
  cancellingId: number | null;
  onCancel: (id: number) => void;
}) {
  const { t } = useApp();
  if (transfers.length === 0) {
    return <p className="muted empty-inline">{t('files.empty')}</p>;
  }
  return (
    <ul className="transfer-list">
      {transfers.map((transfer) => {
        const percent =
          transfer.bytes_total > 0
            ? Math.min(100, Math.round((transfer.bytes_done / transfer.bytes_total) * 100))
            : transfer.state === 'done'
              ? 100
              : 0;
        const cancellable =
          transfer.state === 'offered' ||
          transfer.state === 'transferring' ||
          transfer.state === 'verifying';
        return (
          <li key={transfer.id}>
            <span className="transfer-direction">
              <Icon
                name={transfer.direction === 'send' ? 'updates' : 'download'}
                size={13}
              />
              {transfer.direction === 'send'
                ? t('files.outgoing')
                : t('files.incoming')}
            </span>
            <span className="transfer-name" title={transfer.file_name}>
              {transfer.file_name}
            </span>
            <span className="transfer-meta">
              {formatBytes(transfer.bytes_done)} / {formatBytes(transfer.bytes_total)}
            </span>
            <span className={`transfer-state state-${transfer.state}`}>
              {t(transferStateKey(transfer.state))}
            </span>
            {cancellable ? (
              <button
                type="button"
                className="text-button"
                disabled={cancellingId === transfer.id}
                onClick={() => onCancel(transfer.id)}
              >
                {t('files.cancel')}
              </button>
            ) : null}
            <span className="progress-track" aria-hidden="true">
              <span className="progress-fill" style={{ width: `${percent}%` }} />
            </span>
          </li>
        );
      })}
    </ul>
  );
}
