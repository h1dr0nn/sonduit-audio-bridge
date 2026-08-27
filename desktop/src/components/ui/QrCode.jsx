/**
 * A QR code, rendered in the webview.
 *
 * Generated here rather than in Rust so the backend needs no image encoder and
 * no image ever crosses the IPC boundary: the payload is a short line of text,
 * and the webview already has a renderer for everything else on the page.
 *
 * White quiet zone always, in both themes. A dark surround is what most phone
 * cameras fail on, and a code that only scans in light mode is a code that is
 * broken half the time.
 */

import React, { useEffect, useState } from 'react';
import QRCode from 'qrcode';

export function QrCode({ payload, size = 220, alt }) {
  const [image, setImage] = useState(null);

  useEffect(() => {
    if (!payload) {
      setImage(null);
      return undefined;
    }

    // The payload can change while a render is in flight, and an old image
    // arriving after a new one would show a code that no longer pairs.
    let current = true;

    QRCode.toDataURL(payload, {
      // Medium correction and a four-module quiet zone are what the spec
      // assumes; dropping either makes the symbol smaller and less readable
      // at the distance a laptop screen is actually held from a phone.
      errorCorrectionLevel: 'M',
      margin: 4,
      width: size * 2,
      color: { dark: '#000000', light: '#ffffff' },
    })
      .then((url) => {
        if (current) setImage(url);
      })
      .catch(() => {
        if (current) setImage(null);
      });

    return () => {
      current = false;
    };
  }, [payload, size]);

  if (!image) return null;

  return (
    <img
      src={image}
      alt={alt}
      width={size}
      height={size}
      className="rounded-xl bg-white"
      style={{ imageRendering: 'pixelated' }}
    />
  );
}
