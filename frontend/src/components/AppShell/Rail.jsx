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

function RailButton({ id, Icon, label, active, onSelect }) {
  return (
    <button
      type="button"
      onClick={() => onSelect(id)}
      title={label}
      aria-label={label}
      aria-current={active ? 'page' : undefined}
      className={cn(
        'flex h-11 w-11 items-center justify-center rounded-pill',
        'transition-colors duration-fast ease-out',
        active
          ? 'bg-white text-[#17171a]'
          : 'text-white/55 hover:bg-white/10 hover:text-white/85',
      )}
    >
      <Icon className="h-[18px] w-[18px]" strokeWidth={1.9} />
    </button>
  );
}

export function Rail({ current, onSelect, t }) {
  return (
    <nav className="rail flex flex-col items-center gap-1 px-1.5 py-3">
      {PRIMARY.map((item) => (
        <RailButton
          key={item.id}
          {...item}
          label={t(item.labelKey)}
          active={current === item.id}
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
          onSelect={onSelect}
        />
      ))}
    </nav>
  );
}
