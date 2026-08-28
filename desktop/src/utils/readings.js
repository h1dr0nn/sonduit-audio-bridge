/**
 * How a telemetry number is written down.
 *
 * There is one rule and it lives here, because two components rounding
 * differently is the same class of bug as two places deciding which link a
 * session is on: both produce a screen that contradicts itself, and neither
 * half looks wrong on its own.
 *
 * Both readouts use it -- the tiles on the connection and telemetry pages, and
 * the compact lines in the titlebar status menu.
 */

/**
 * Round a reading to the precision the eye can use.
 *
 * A raw double reached the screen as `1557.430087`, which overflowed its tile
 * and implied a precision the measurement does not have. It happened again in
 * the status menu as `37.3735737102045 ms`, which is why this is no longer
 * private to one component. Milliseconds are read whole; a percentage below
 * one still needs its decimals, because 0.4% and 0.0% mean different things.
 */
export function formatReading(value, unit) {
  if (typeof value !== 'number') return value;
  if (!Number.isFinite(value)) return '—';

  if (unit === '%') {
    return value >= 10 ? value.toFixed(0) : value.toFixed(2);
  }
  if (unit === 'ppm') {
    return value.toFixed(1);
  }
  return Math.abs(value) >= 10 ? value.toFixed(0) : value.toFixed(1);
}

/**
 * The same reading with its unit written after it, as one string.
 *
 * For a readout that has no room to render the unit separately. `null` for a
 * value the far end has never reported, which the caller renders as absent
 * rather than as a zero: until a receiver answers there is nothing measured.
 */
export function formatWithUnit(value, unit) {
  if (value === null || value === undefined) return null;

  const shown = formatReading(value, unit);
  // Nothing sensible to put a unit after. An em dash followed by "ms" reads as
  // a broken number rather than as an absent one.
  if (shown === '—' || !unit) return String(shown);
  return `${shown} ${unit}`;
}
