import { useState } from 'react';
import type { TelemetryConsentState } from '../types';
import { errorToMessage } from '../tauri';
import { useI18n } from '../useI18n';

export function TelemetryConsentDialog({
  onChoose,
}: {
  onChoose: (enabled: boolean) => Promise<TelemetryConsentState>;
}) {
  const { t } = useI18n();
  const [savingChoice, setSavingChoice] = useState<boolean | null>(null);
  const [error, setError] = useState<string | null>(null);

  const handleChoose = (enabled: boolean) => {
    setSavingChoice(enabled);
    setError(null);
    void onChoose(enabled)
      .catch((chooseError) => {
        setError(errorToMessage(chooseError, t));
      })
      .finally(() => setSavingChoice(null));
  };

  return (
    <div className="modal-backdrop" role="presentation">
      <div className="consent-dialog" role="dialog" aria-modal="true" aria-labelledby="telemetry-title">
        <h2 id="telemetry-title">{t('telemetry.title')}</h2>
        <p>{t('telemetry.description')}</p>
        <div className="consent-facts">
          <span>{t('telemetry.factLocal')}</span>
          <span>{t('telemetry.factHashed')}</span>
          <span>{t('telemetry.factDisable')}</span>
        </div>
        <div className="consent-actions">
          <button className="btn-secondary" disabled={savingChoice !== null} onClick={() => handleChoose(false)}>
            {savingChoice === false ? t('telemetry.saving') : t('telemetry.keepOff')}
          </button>
          <button className="btn-primary" disabled={savingChoice !== null} onClick={() => handleChoose(true)}>
            {savingChoice === true ? t('telemetry.saving') : t('telemetry.enable')}
          </button>
        </div>
        {error && <p className="consent-error">{error}</p>}
      </div>
    </div>
  );
}
