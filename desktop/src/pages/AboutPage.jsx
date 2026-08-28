import React from 'react';
import { Card } from '../components/ui/Card';
import { useTranslation } from '../i18n';

const APP_VERSION = __APP_VERSION__;
const REPOSITORY = 'github.com/h1dr0nn/sonduit-audio-bridge';

/**
 * The author, spelled as the project's own copyright line spells it.
 *
 * `LICENSE` reads "Copyright (c) 2025 h1dr0n", and that is the name a credit
 * owes. The repository URL above carries `h1dr0nn` with two n's because that
 * is the GitHub account, not the name; the two are deliberately different here
 * rather than one of them being a typo.
 */
const AUTHOR = 'h1dr0n';

/**
 * Third-party software that ships inside the installer.
 *
 * FFmpeg is here because the LGPL requires it: an application that distributes
 * an LGPL binary has to say so and has to carry the licence text. Sonduit
 * spawns ffmpeg as a separate process and never links against it, which is
 * what keeps this project MIT, but the notice obligation applies either way.
 * See docs/licensing.md section 2.2.
 */
const BUNDLED = [
  {
    name: 'FFmpeg',
    licence: 'LGPL 3.0',
    detail: 'ffmpeg.exe, unmodified, from the BtbN LGPL build',
    source: 'github.com/BtbN/FFmpeg-Builds',
  },
];

/**
 * Projects this one was read against and does not ship.
 *
 * They are a separate list from the bundled one because the legal meaning is
 * the opposite: nothing here is distributed, and for the two copyleft entries
 * that is the only reason an MIT product may exist beside them at all. Each
 * note says what was actually read, since "referenced" on its own would blur
 * the one distinction that matters.
 *
 * The licence identifiers and the wording are taken from docs/licensing.md
 * sections 2 and 3 and docs/protocol.md, which are where the analysis lives.
 * Scream's own driver source is MS-PL, not GPL: the whole protocol was
 * derived from it precisely because of that, and calling it GPL here would
 * misrepresent the decision the project made.
 */
const STUDIED = [
  {
    name: 'Scream',
    licence: 'MS-PL',
    noteKey: 'about.studiedScream',
    source: 'github.com/duncanthrax/scream',
  },
  {
    name: 'scream-android',
    licence: 'GPL-3.0',
    noteKey: 'about.studiedScreamAndroid',
    source: 'github.com/martinellimarco/scream-android',
  },
  {
    name: 'AndroidUsbAudioDevice',
    licenceKey: 'about.noLicence',
    noteKey: 'about.studiedUsbAudio',
    source: 'github.com/BreadFish64/AndroidUsbAudioDevice',
  },
];

/**
 * The crates the audio path is built on.
 *
 * Names and version requirements are the ones in each crate's own
 * `Cargo.toml`, not resolved lock versions: a resolved version
 * would be wrong the moment `cargo update` ran, and this screen has no way to
 * know that happened. Licences are deliberately absent, because the generated
 * notice named below is the only complete and current answer to that question.
 */
const AUDIO_PATH = [
  { name: 'rubato', version: '0.16', roleKey: 'about.depRubato' },
  { name: 'rtrb', version: '0.4', roleKey: 'about.depRtrb' },
  { name: 'hmac / sha2', version: '0.12 / 0.10', roleKey: 'about.depCrypto' },
  { name: 'windows', version: '0.62', roleKey: 'about.depWindows' },
  { name: 'ndk', version: '0.9', roleKey: 'about.depNdk' },
];

/**
 * What this build is and what it needs, in the order a user would ask.
 *
 * Every value is copied from the file that decides it, and the source is named
 * against each one so the next person can check it rather than trust it: a
 * specification screen that has drifted from the product is worse than one
 * that says nothing. Nothing here is measured at runtime, and the latency
 * figures are budgets rather than measurements, which is what
 * docs/latency-budget.md calls them.
 */
const SPECIFICATION = [
  // docs/protocol.md section 8, baseline format row. The packet size and rate
  // beside it in that table are left out on purpose: they say nothing to
  // anyone who is not implementing the protocol, and this column is short
  // enough already without them.
  { labelKey: 'about.stream', value: '48 kHz, 16-bit, stereo' },
  // crates/sonduit-core/src/jitter.rs, JitterPolicy::for_transport. The
  // latency budget from docs/latency-budget.md used to sit below this one and
  // was dropped: two pairs of millisecond figures one above the other invited
  // being read as one measurement disagreeing with itself.
  { labelKey: 'about.bufferTarget', value: '30 ms Wi-Fi / 10 ms USB' },
  // ADR-002 implements Windows 10 as well as 11; android/gradle.properties
  // pins minSdk 27, which is Android 8.1.
  { labelKey: 'about.requires', value: 'Windows 10+, Android 8.1+' },
];

/* One fact in the left column, at the height its own text needs.
 *
 * An earlier version divided the column between three panels instead, which
 * gave each of them two lines of text and two hundred pixels of box. Filling a
 * column is a question of having enough to say in it, not of inflating what is
 * there. */
const FACT = 'card-sunken shrink-0 px-4 py-3';

/* One entry in a list of software. */
const ENTRY = 'card-sunken px-4 py-3';

export function AboutPage() {
  const { t } = useTranslation();

  return (
    /* Two columns, both claiming the full height and both scrolling their own
     * contents: what this build is on the left, what it owes other people on
     * the right. Splitting them means the attribution list can grow without
     * pushing the version number off the screen.
     *
     * `min-h-0` is on every flex and grid child in the chain because a child
     * of either defaults to `min-height: auto` and will not shrink below its
     * own content, which pushes the overflow out to the shell. */
    <div className="flex h-full min-h-0 flex-col gap-4">
      <header className="shrink-0 px-1">
        <h1 className="text-3xl font-semibold tracking-tight text-ink">{t('about.title')}</h1>
        <p className="mt-1 text-sm text-ink-soft">{t('about.tagline')}</p>
      </header>

      {/* The left column is sized to the repository URL rather than to a round
        * number: it is the longest unbreakable thing on the page, and at the
        * narrower width it used to have it wrapped onto a second line. */}
      <div className="grid min-h-0 flex-1 grid-cols-[minmax(360px,420px)_1fr] gap-4">
        <Card className="min-h-0">
          <dl className="scroll-area flex min-h-0 flex-1 flex-col gap-2">
            <div className={FACT}>
              <dt className="text-xs uppercase tracking-wide text-ink-faint">
                {t('settings.version')}
              </dt>
              <dd className="mt-1 font-mono text-lg tabular-nums text-ink">{APP_VERSION}</dd>
            </div>
            <div className={FACT}>
              <dt className="text-xs uppercase tracking-wide text-ink-faint">
                {t('about.license')}
              </dt>
              <dd className="mt-1 font-mono text-lg text-ink">MIT</dd>
            </div>
            <div className={FACT}>
              <dt className="text-xs uppercase tracking-wide text-ink-faint">
                {t('about.author')}
              </dt>
              <dd className="mt-1 font-mono text-lg text-ink">{AUTHOR}</dd>
            </div>
            <div className={FACT}>
              <dt className="text-xs uppercase tracking-wide text-ink-faint">
                {t('about.repository')}
              </dt>
              {/* One line, never broken. The column is sized so that it fits:
                * half a repository address is no use to anyone trying to type
                * it in, and a URL split across two lines reads as two. */}
              <dd className="mt-1 whitespace-nowrap font-mono text-xs text-ink">{REPOSITORY}</dd>
            </div>
            {SPECIFICATION.map((fact) => (
              <div key={fact.labelKey} className={FACT}>
                <dt className="text-xs uppercase tracking-wide text-ink-faint">
                  {t(fact.labelKey)}
                </dt>
                <dd className="mt-1 font-mono text-sm text-ink">{fact.value}</dd>
              </div>
            ))}
          </dl>
        </Card>

        {/* Attribution on the right. It is longer than any window this
          * application can be given, so it is the part that scrolls, and it
          * scrolls here rather than dragging the shell with it. */}
        <div className="scroll-area flex min-h-0 min-w-0 flex-col gap-4">
          <Card className="shrink-0" title={t('about.bundled')} subtitle={t('about.bundledNote')}>
            <ul className="flex flex-col gap-2">
              {BUNDLED.map((item) => (
                <li key={item.name} className={ENTRY}>
                  <div className="flex items-baseline justify-between gap-3">
                    <span className="text-sm font-medium text-ink">{item.name}</span>
                    <span className="font-mono text-xs text-ink-soft">{item.licence}</span>
                  </div>
                  <p className="mt-1 text-xs text-ink-soft">{item.detail}</p>
                  <p className="mt-0.5 font-mono text-xs text-ink-faint">{item.source}</p>
                </li>
              ))}
            </ul>
          </Card>

          <Card className="shrink-0" title={t('about.studied')} subtitle={t('about.studiedNote')}>
            <ul className="flex flex-col gap-2">
              {STUDIED.map((item) => (
                <li key={item.name} className={ENTRY}>
                  <div className="flex items-baseline justify-between gap-3">
                    <span className="text-sm font-medium text-ink">{item.name}</span>
                    <span className="font-mono text-xs text-ink-soft">
                      {item.licence ?? t(item.licenceKey)}
                    </span>
                  </div>
                  <p className="mt-1 text-xs text-ink-soft">{t(item.noteKey)}</p>
                  <p className="mt-0.5 font-mono text-xs text-ink-faint">{item.source}</p>
                </li>
              ))}
            </ul>
          </Card>

          <Card
            className="shrink-0"
            title={t('about.dependencies')}
            subtitle={t('about.dependenciesNote')}
          >
            <ul className="flex flex-col gap-2">
              {AUDIO_PATH.map((item) => (
                <li key={item.name} className={ENTRY}>
                  <div className="flex items-baseline justify-between gap-3">
                    <span className="font-mono text-sm font-medium text-ink">{item.name}</span>
                    <span className="font-mono text-xs tabular-nums text-ink-soft">
                      {item.version}
                    </span>
                  </div>
                  <p className="mt-1 text-xs text-ink-soft">{t(item.roleKey)}</p>
                </li>
              ))}
            </ul>
          </Card>

          <p className="shrink-0 px-1 pb-1 text-xs text-ink-faint">{t('about.licenceLocation')}</p>
        </div>
      </div>
    </div>
  );
}
