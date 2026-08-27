import React, { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';

/** Long enough that sweeping the pointer across a toolbar stays silent. */
const OPEN_DELAY_MS = 420;

/** Distance from the trigger, and the smallest gap kept to the window edge. */
const GAP = 8;
const EDGE = 8;

function callBoth(theirs, ours) {
  return (event) => {
    theirs?.(event);
    ours(event);
  };
}

/**
 * Place the tip on `side`, flipping to the opposite side when it would leave
 * the window, then clamping along the free axis. Flipping first keeps the tip
 * off the trigger it describes; clamping alone would slide it underneath.
 */
function place(trigger, tip, side) {
  const { innerWidth: vw, innerHeight: vh } = window;
  let resolved = side;

  if (side === 'bottom' && trigger.bottom + GAP + tip.height > vh - EDGE) resolved = 'top';
  else if (side === 'top' && trigger.top - GAP - tip.height < EDGE) resolved = 'bottom';
  else if (side === 'right' && trigger.right + GAP + tip.width > vw - EDGE) resolved = 'left';
  else if (side === 'left' && trigger.left - GAP - tip.width < EDGE) resolved = 'right';

  let top;
  let left;
  if (resolved === 'top' || resolved === 'bottom') {
    top = resolved === 'top' ? trigger.top - GAP - tip.height : trigger.bottom + GAP;
    left = trigger.left + (trigger.width - tip.width) / 2;
  } else {
    left = resolved === 'left' ? trigger.left - GAP - tip.width : trigger.right + GAP;
    top = trigger.top + (trigger.height - tip.height) / 2;
  }

  return {
    top: Math.min(Math.max(EDGE, top), Math.max(EDGE, vh - tip.height - EDGE)),
    left: Math.min(Math.max(EDGE, left), Math.max(EDGE, vw - tip.width - EDGE)),
  };
}

/**
 * Tooltip for a single interactive child.
 *
 * The child keeps its own `aria-label`: that is what assistive technology
 * announces, so the tip is decoration and stays out of the accessibility tree.
 * A native `title` attribute is deliberately not used anywhere in this app —
 * it is browser chrome, it cannot be styled, and it ignores keyboard focus.
 */
export function Tooltip({ label, side = 'bottom', disabled = false, children }) {
  const child = React.Children.only(children);
  const triggerRef = useRef(null);
  const tipRef = useRef(null);
  const timer = useRef(null);
  const [open, setOpen] = useState(false);
  const [position, setPosition] = useState(null);

  const close = useCallback(() => {
    window.clearTimeout(timer.current);
    setOpen(false);
    setPosition(null);
  }, []);

  const openLater = useCallback(() => {
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => setOpen(true), OPEN_DELAY_MS);
  }, []);

  useEffect(() => close, [close]);
  useEffect(() => {
    if (disabled) close();
  }, [disabled, close]);

  // Measured after the tip is in the DOM, because placement needs its size.
  useLayoutEffect(() => {
    if (!open || !triggerRef.current || !tipRef.current) return;
    setPosition(
      place(
        triggerRef.current.getBoundingClientRect(),
        tipRef.current.getBoundingClientRect(),
        side,
      ),
    );
  }, [open, side, label]);

  useEffect(() => {
    if (!open) return undefined;
    window.addEventListener('resize', close);
    window.addEventListener('blur', close);
    window.addEventListener('scroll', close, true);
    return () => {
      window.removeEventListener('resize', close);
      window.removeEventListener('blur', close);
      window.removeEventListener('scroll', close, true);
    };
  }, [open, close]);

  if (disabled || !label) return child;

  const trigger = React.cloneElement(child, {
    ref: (node) => {
      triggerRef.current = node;
      const forwarded = child.ref;
      if (typeof forwarded === 'function') forwarded(node);
      else if (forwarded) forwarded.current = node;
    },
    onMouseEnter: callBoth(child.props.onMouseEnter, openLater),
    onMouseLeave: callBoth(child.props.onMouseLeave, close),
    // A tip left hanging over a menu the click just opened reads as a glitch.
    onMouseDown: callBoth(child.props.onMouseDown, close),
    onFocus: callBoth(child.props.onFocus, openLater),
    onBlur: callBoth(child.props.onBlur, close),
  });

  return (
    <>
      {trigger}
      {open &&
        createPortal(
          <div
            ref={tipRef}
            className="tooltip"
            aria-hidden="true"
            data-placed={position ? '' : undefined}
            // Rendered invisibly for one frame so its size can be measured
            // before it is placed, which stops a visible jump on first show.
            style={position ?? { top: 0, left: 0, visibility: 'hidden' }}
          >
            {label}
          </div>,
          document.body,
        )}
    </>
  );
}
