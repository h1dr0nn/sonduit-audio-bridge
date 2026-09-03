import React, { useCallback, useEffect, useMemo, useState } from 'react';
import {
  FiLock,
  FiMaximize,
  FiPlay,
  FiRefreshCw,
  FiSmartphone,
  FiSpeaker,
  FiSquare,
  FiUnlock,
} from 'react-icons/fi';
import { Button } from '../components/ui/Button';
import { Card } from '../components/ui/Card';
import { showToast } from '../components/ui/Toast';
import { Dialog } from '../components/ui/Dialog';
import { QrCode } from '../components/ui/QrCode';
import { StatTile } from '../components/ui/StatTile';
import { TextField } from '../components/ui/TextField';
import { useSettingsContext } from '../context/SettingsContext';
import { useBridge } from '../hooks/useBridge';
import { useTranslation } from '../i18n';

/**
 * A device the user has picked, or an address typed by hand.
 *
 * Kept in one piece of state rather than two so the two cannot both be set,
 * which would leave the start button unable to say what it would do.
 */
const NO_TARGET = { kind: 'multicast', value: null };

/**
 * How long an invite stays open, in seconds.
 *
 * Matches the window `bridge::await_pairing` waits for. Shown as a countdown
 * so the code visibly has a life: one that never expires is a secret left on
 * a screen indefinitely.
 */
const PAIRING_WINDOW_SECONDS = 90;

export function ConnectionPage() {
  const { t } = useTranslation();
  // Read here rather than in the backend's own store: the preference belongs
  // to the session that is being started, so it travels with the start call.
  const { settings } = useSettingsContext();
  const {
    available,
    status,
    error,
    devices,
    session,
    telemetry,
    scan,
    invite,
    awaitPairing,
    cancelPairing,
    start,
    stop,
  } = useBridge();

  const [target, setTarget] = useState(NO_TARGET);
  const [typed, setTyped] = useState('');
  const [pairing, setPairing] = useState('');
  const [scanning, setScanning] = useState(false);
  const [invitation, setInvitation] = useState(null);
  const [pairingState, setPairingState] = useState('idle');
  const [expiresIn, setExpiresIn] = useState(null);
  const [busy, setBusy] = useState(false);

  const running = status === 'connected' || status === 'connecting';

  /*
   * A session can only be started against a device this run has paired with.
   *
   * The backend refuses to send Sonduit audio it cannot encrypt, and every
   * device in the list above agreed a key on its way into that list. A typed
   * address and the multicast group have no key and never had one, so the
   * button says why instead of failing after it is pressed. See ADR-009.
   */
  const canStart = target.kind === 'device';

  const targetLabel = useMemo(() => {
    if (target.kind === 'device') return target.value;
    if (target.kind === 'manual') return typed.trim() || t('connection.multicast');
    return t('connection.multicast');
  }, [target, typed, t]);

  /**
   * A phone that has been found is the phone the user meant.
   *
   * Without this, finding a device left the target on the multicast group and
   * said nothing about it: the session started, packets went out, and the
   * phone on screen received none of them, because it was never joined to that
   * group. Six hundred packets sent with no latency and no loss reported is
   * what that looks like, and it reads as the app being broken.
   *
   * Only ever fills in a blank. A user who has picked a device, typed an
   * address, or deliberately chosen the group keeps their choice; this fires
   * once, on the first device to appear, and not again.
   */
  useEffect(() => {
    if (devices.length === 0) return;
    setTarget((current) =>
      current.kind === 'multicast' && current.value === null
        ? { kind: 'device', value: devices[0].address }
        : current,
    );
  }, [devices]);

  // The bridge can fail long after the button that started it has returned --
  // the capture thread dying, the receiver going away. Raised here because
  // there is no call to attach it to.
  useEffect(() => {
    if (!error) return;
    showToast({ id: 'bridge', tone: 'error', titleKey: 'connection.bridgeError', detail: String(error) });
  }, [error]);

  const handleScan = useCallback(async () => {
    setScanning(true);
    try {
      await scan(pairing);
    } catch (reason) {
      showToast({ id: 'scan', tone: 'error', titleKey: 'connection.scanFailed', detail: String(reason) });
    } finally {
      setScanning(false);
    }
  }, [scan, pairing]);

  /**
   * Put a fresh code on screen and wait for a phone to read it.
   *
   * The wait is started in the same action rather than behind a second button:
   * the code is only valid for one pairing window, and a user who showed one
   * and then had to press something else to arm it would be looking at a code
   * that does nothing.
   */
  const handleShowInvite = useCallback(async () => {
    setPairingState('waiting');
    setExpiresIn(PAIRING_WINDOW_SECONDS);
    try {
      const next = await invite();
      setInvitation(next);
      const device = await awaitPairing();
      if (device) {
        setPairingState('paired');
        showToast({ id: 'pair', tone: 'success', titleKey: 'connection.paired', message: device.name });
        // Selected as well as listed. The user asked for this phone by
        // pointing a camera at the screen; making them click it again would be
        // asking the same question twice.
        setTarget({ kind: 'device', value: device.address });
      } else {
        setPairingState('timeout');
      }
    } catch (reason) {
      showToast({ id: 'pair', tone: 'error', titleKey: 'connection.pairFailed', detail: String(reason) });
      setPairingState('idle');
      setInvitation(null);
    } finally {
      setExpiresIn(null);
    }
  }, [invite, awaitPairing]);

  /**
   * Closing the dialog ends the code's life.
   *
   * A pairing code is a shared secret with a purpose; one that stays valid
   * after it has been used, or after the user has stopped looking at it, is a
   * secret sitting on a screen for no reason. The backend generates a fresh
   * one for every invite, so throwing this away costs nothing.
   */
  const handleCloseInvite = useCallback(() => {
    setInvitation(null);
    setPairingState('idle');
    setExpiresIn(null);
    // Tells the backend as well as the screen. It holds the discovery port for
    // the whole window, and until it lets go, showing a second code fails.
    cancelPairing().catch(() => {
      // Nothing useful to say: the window closes on its own regardless.
    });
  }, [cancelPairing]);

  // Counts the window down so the user can see the code is not permanent.
  // Purely a display: the backend stops listening on its own.
  useEffect(() => {
    if (pairingState !== 'waiting' || expiresIn === null) return undefined;
    if (expiresIn <= 0) return undefined;
    const timer = setTimeout(() => setExpiresIn((left) => (left ?? 1) - 1), 1000);
    return () => clearTimeout(timer);
  }, [pairingState, expiresIn]);

  // A code that has been used has done its job. Leaving the dialog open with a
  // live code invites a second, unwanted device.
  useEffect(() => {
    if (pairingState !== 'paired') return undefined;
    const timer = setTimeout(handleCloseInvite, 1500);
    return () => clearTimeout(timer);
  }, [pairingState, handleCloseInvite]);

  const handleStart = useCallback(async () => {
    setBusy(true);
    try {
      const address = target.kind === 'manual' ? typed.trim() : target.value;
      await start({
        target: address || null,
        preferredTransport: settings.preferredTransport,
        captureDeviceId: settings.captureDeviceId,
      });
    } catch (reason) {
      showToast({ id: 'session', tone: 'error', titleKey: 'connection.startFailed', detail: String(reason) });
    } finally {
      setBusy(false);
    }
  }, [settings.captureDeviceId, settings.preferredTransport, start, target, typed]);

  const handleStop = useCallback(async () => {
    setBusy(true);
    try {
      await stop();
    } catch (reason) {
      showToast({ id: 'session', tone: 'error', titleKey: 'connection.stopFailed', detail: String(reason) });
    } finally {
      setBusy(false);
    }
  }, [stop]);

  return (
    // Full height with the columns scrolling their own contents, matching the
    // editor. min-h-0 on every flex and grid child in the chain is what makes
    // that work: a flex child defaults to min-height auto and will not shrink.
    <div className="flex h-full min-h-0 flex-col gap-4">
      <header className="flex items-end justify-between gap-4 px-1">
        <div>
          <h1 className="text-3xl font-semibold tracking-tight text-ink">
            {t('connection.title')}
          </h1>
          <p className="mt-1 text-sm text-ink-soft">{t('connection.subtitle')}</p>
        </div>
        <div className="flex items-center gap-3">
          {/* No status pill here: the titlebar carries the same state, and two
              of them side by side is one too many. */}
          {running ? (
            <Button variant="quiet" icon={FiSquare} disabled={busy} onClick={handleStop}>
              {t('connection.stop')}
            </Button>
          ) : (
            <Button
              variant="accent"
              icon={FiPlay}
              disabled={!available || busy || !canStart}
              onClick={handleStart}
            >
              {busy ? t('connection.starting') : t('connection.start')}
            </Button>
          )}
        </div>
      </header>

      <div className="grid min-h-0 flex-1 grid-cols-[minmax(280px,1fr)_minmax(300px,380px)] gap-4">
        {/* One card, so it takes the whole column rather than sitting at the
          * top of a taller box. min-h-0 all the way down: a flex child
          * defaults to min-height auto and refuses to shrink below its
          * content, which is what stops an inner scroll area from ever
          * scrolling. */}
        <div className="flex min-h-0 min-w-0 flex-col gap-4">
        <Card
          className="min-h-0 flex-1"
          title={t('connection.devices')}
          actions={
            <div className="flex items-center gap-2">
              <Button
                size="sm"
                icon={FiMaximize}
                disabled={!available}
                onClick={handleShowInvite}
              >
                {t('connection.showQr')}
              </Button>
              <Button
                size="sm"
                icon={FiRefreshCw}
                disabled={!available || scanning || pairing.replace(/\D/g, '').length !== 6}
                onClick={handleScan}
              >
                {scanning ? t('connection.scanning') : t('connection.scan')}
              </Button>
            </div>
          }
        >
          <div className="scroll-area min-h-0 flex-1">
          {devices.length === 0 ? (
            <div className="flex h-full flex-col items-center justify-center gap-2 py-8 text-center">
              <FiSmartphone className="h-7 w-7 text-ink-faint" strokeWidth={1.5} />
              <p className="text-sm text-ink-soft">{t('connection.noDevices')}</p>
              {!available && <p className="text-xs text-ink-faint">{t('connection.captureOnly')}</p>}
            </div>
          ) : (
            <ul className="flex flex-col gap-2">
              {devices.map((device) => {
                const selected = target.kind === 'device' && target.value === device.address;
                return (
                  <li
                    key={device.id}
                    className="card-sunken flex items-center justify-between px-4 py-3"
                  >
                    <div className="min-w-0">
                      <p className="truncate text-sm font-medium text-ink">{device.name}</p>
                      <p className="truncate font-mono text-xs text-ink-soft">{device.address}</p>
                    </div>
                    <Button
                      size="sm"
                      variant={selected ? 'primary' : 'quiet'}
                      disabled={running}
                      onClick={() => setTarget({ kind: 'device', value: device.address })}
                    >
                      {t('connection.connect')}
                    </Button>
                  </li>
                );
              })}
            </ul>
          )}
          </div>

          <div className="mt-4 flex flex-none flex-col gap-4 border-t border-line-soft pt-4">
            <TextField
              id="pairing-code"
              label={t('connection.pairingCode')}
              hint={t('connection.pairingHint')}
              placeholder="000000"
              inputMode="numeric"
              maxLength={7}
              value={pairing}
              disabled={running}
              onChange={(event) => setPairing(event.target.value)}
            />
            <TextField
              id="manual-address"
              label={t('connection.address')}
              placeholder={t('connection.addressHint')}
              value={typed}
              disabled={running}
              onChange={(event) => {
                setTyped(event.target.value);
                setTarget(event.target.value.trim() ? { kind: 'manual', value: null } : NO_TARGET);
              }}
            />
            {!running && !canStart && (
              <p className="-mt-2 text-xs text-ink-faint">{t('connection.pairToStart')}</p>
            )}
          </div>
        </Card>

        </div>

        {/* Session keeps its natural height -- it is a fixed set of facts --
          * and telemetry takes whatever is left, so the column reaches the
          * bottom without either card being stretched out of proportion. */}
        <div className="flex min-h-0 min-w-0 flex-col gap-4">
        <Card className="flex-none" tone="accent" title={t('connection.session')}>
          <div className="mt-auto flex flex-col gap-3">
            <div className="flex items-center gap-2 opacity-90">
              <FiSpeaker className="h-5 w-5" strokeWidth={1.75} />
              <span className="text-sm">{t('connection.captureSource')}</span>
            </div>
            <p className="font-mono text-sm opacity-80">
              {session.endpoint ?? t('connection.noEndpoint')}
            </p>

            <div className="text-xs opacity-70">{t('connection.target')}</div>
            <p className="-mt-2 truncate font-mono text-sm opacity-80">
              {session.target ?? targetLabel}
            </p>

            {/* Whether the audio going out is encrypted, from the backend and
              * not from anything this page infers. A session that is not
              * encrypted must never look like one that is, so the state is
              * shown for a running session either way rather than only when
              * it is the good news. */}
            {session.encrypted !== null && session.encrypted !== undefined && (
              <div className="flex items-center gap-2">
                {session.encrypted ? (
                  <FiLock className="h-4 w-4" strokeWidth={1.75} />
                ) : (
                  <FiUnlock className="h-4 w-4" strokeWidth={1.75} />
                )}
                <span className="text-sm opacity-90">
                  {session.encrypted
                    ? t('connection.encryptionOn')
                    : t('connection.encryptionOff')}
                </span>
              </div>
            )}

            {/* Zero on a healthy link. Anything else is somebody on the network
              * sending status datagrams this session refused, which is the one
              * thing the user could not otherwise find out. */}
            {telemetry.refusedReports > 0 && (
              <p className="-mt-1 text-xs opacity-80">
                {t('telemetry.refusedReports')}: {telemetry.refusedReports}
              </p>
            )}

            {/* The record of faults that healed. The status line above states
              * a failure only while it is still true -- a send error stops
              * being shown the moment a datagram gets through -- so without
              * these counts a link that refused four hundred datagrams and
              * recovered from every one would look identical to one that
              * never faltered. Absent while zero: a healthy session should
              * not have to read two zeroes to learn that nothing is wrong. */}
            {telemetry.sendFailures > 0 && (
              <p className="-mt-1 text-xs opacity-80">
                {t('telemetry.sendFailures')}: {telemetry.sendFailures}
              </p>
            )}
            {telemetry.captureFailures > 0 && (
              <p className="-mt-1 text-xs opacity-80">
                {t('telemetry.captureFailures')}: {telemetry.captureFailures}
              </p>
            )}

            <dl className="mt-2 grid grid-cols-3 gap-2 border-t border-white/20 pt-3 text-xs">
              <div>
                <dt className="opacity-70">{t('connection.sampleRate')}</dt>
                <dd className="mt-0.5 font-mono text-sm">{session.sampleRate ?? '—'}</dd>
              </div>
              <div>
                <dt className="opacity-70">{t('connection.channels')}</dt>
                <dd className="mt-0.5 font-mono text-sm">{session.channels ?? '—'}</dd>
              </div>
              <div>
                <dt className="opacity-70">{t('connection.bitDepth')}</dt>
                <dd className="mt-0.5 font-mono text-sm">{session.bitDepth ?? '—'}</dd>
              </div>
            </dl>
          </div>
        </Card>
        <Card
          className="min-h-0 flex-1"
          title={t('telemetry.title')}
          subtitle={t('telemetry.subtitle')}
        >
        {/* The tiles share the card's spare height between them rather than
          * sitting in a band at the top of an empty card. */}
        <div className="grid min-h-0 flex-1 grid-cols-2 grid-rows-2 gap-3">
          <StatTile label={t('telemetry.latency')} value={telemetry.latencyMs} unit="ms" />
          <StatTile label={t('telemetry.bufferDepth')} value={telemetry.bufferDepthMs} unit="ms" />
          <StatTile label={t('telemetry.packetLoss')} value={telemetry.packetLossPct} unit="%" />
          <StatTile label={t('telemetry.packetsSent')} value={telemetry.packetsSent} />
        </div>
        {!running && (
          <p className="mt-4 flex-none text-center text-xs text-ink-faint">
            {t('telemetry.noData')}
          </p>
        )}
        </Card>
        </div>
      </div>

      <Dialog
        open={invitation !== null}
        title={t('connection.pairPhone')}
        confirmLabel={t('close')}
        onClose={handleCloseInvite}
      >
        <div className="flex flex-col items-center gap-4">
          <QrCode payload={invitation?.payload ?? ''} alt={t('connection.showQr')} />

          <p className="text-center text-sm text-ink-soft">{t('connection.qrHint')}</p>

          <div className="w-full">
            <p className="text-xs uppercase tracking-wide text-ink-faint">
              {t('connection.qrAddresses')}
            </p>
            <ul className="mt-1 flex flex-col gap-0.5">
              {(invitation?.addresses ?? []).map((address) => (
                <li key={address} className="truncate font-mono text-sm text-ink-soft">
                  {address}
                </li>
              ))}
            </ul>
          </div>

          <p className="text-center text-sm text-ink">
            {pairingState === 'waiting' && t('connection.qrWaiting')}
            {pairingState === 'paired' && t('connection.qrPaired')}
            {pairingState === 'timeout' && t('connection.qrTimeout')}
          </p>

          {expiresIn !== null && pairingState === 'waiting' && (
            <p className="text-xs text-ink-faint">
              {t('connection.qrExpires', { seconds: expiresIn })}
            </p>
          )}
        </div>
      </Dialog>
    </div>
  );
}
