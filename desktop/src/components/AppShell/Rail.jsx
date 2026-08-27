import React from 'react';
import { FiActivity, FiHelpCircle, FiRadio, FiSettings } from 'react-icons/fi';
import { cn } from '../../utils/cn';

const PRIMARY = [
  { id: 'connection', Icon: FiRadio, labelKey: 'nav.connection' },
  { id: 'telemetry', Icon: FiActivity, labelKey: 'nav.telemetry' },
];

const SECONDARY = [
  { id: 'settings', Icon: FiSettings, labelKey: 'nav.settings' },
  { id: 'about', Icon: FiHelpCircle, labelKey: 'nav.about' },
];

function RailButton({ id, Icon, label, active, expanded, onSelect }) {
  return (
    <button
      type="button"
      onClick={() => onSelect(id)}
      title={expanded ? undefined : label}
      aria-label={label}
      aria-current={active ? 'page' : undefined}
      className={cn(
        'flex h-11 items-center transition-colors duration-fast ease-out',
        expanded ? 'w-full gap-3 rounded-[14px] px-3' : 'w-11 justify-center rounded-pill',
        active ? 'bg-white text-[#17171a]' : 'text-white/55 hover:bg-white/10 hover:text-white/85',
      )}
    >
      <Icon className="h-[18px] w-[18px] flex-none" strokeWidth={1.9} />
      {expanded && <span className="truncate text-sm font-medium">{label}</span>}
    </button>
  );
}

export function Rail({ current, onSelect, expanded, t }) {
  return (
    <nav
      className={cn(
        'rail flex flex-col items-center gap-1',
        // Width is animated rather than toggled so the main pane reflows
        // smoothly instead of snapping.
        'transition-[width] duration-normal ease-out',
        // Padding is equal on all four sides, and the inner radius is the outer
        // radius minus that padding, so the active item sits concentrically
        // inside the rail instead of cutting its corner.
        //   collapsed: pill outside, 6px padding, pill inside
        //   expanded:  22px outside, 8px padding, 14px inside
        expanded ? 'w-52 rounded-card p-2' : 'w-14 rounded-pill p-1.5',
      )}
    >
      {PRIMARY.map((item) => (
        <RailButton
          key={item.id}
          {...item}
          label={t(item.labelKey)}
          active={current === item.id}
          expanded={expanded}
          onSelect={onSelect}
        />
      ))}
      <div className="flex-1" />
      {SECONDARY.map((item) => (
        <RailButton
          key={item.id}
          {...item}
          label={t(item.labelKey)}
          active={current === item.id}
          expanded={expanded}
          onSelect={onSelect}
        />
      ))}
    </nav>
  );
}
