import type { ButtonHTMLAttributes, ReactNode } from "react";
import { ChevronRight } from "./icons";

interface DisclosureContentProps {
  label: ReactNode;
  labelId?: string;
  icon?: ReactNode;
}

function DisclosureContent({ label, labelId, icon }: DisclosureContentProps) {
  return <>
    {icon}
    <ChevronRight aria-hidden="true" className="section-disclosure-chevron" size={14} />
    <span className="section-disclosure-label" data-button-content id={labelId}>{label}</span>
  </>;
}

export function DisclosureSummary(props: DisclosureContentProps) {
  return <summary className="section-disclosure" data-pointer-glow="off">
    <DisclosureContent {...props} />
  </summary>;
}

interface DisclosureButtonProps extends DisclosureContentProps, ButtonHTMLAttributes<HTMLButtonElement> {
  "aria-expanded": boolean;
}

export function DisclosureButton({ label, labelId, icon, className = "", children, ...props }: DisclosureButtonProps) {
  return <button {...props} className={`section-disclosure ${className}`.trimEnd()} data-pointer-glow="off" type="button">
    <DisclosureContent icon={icon} label={label} labelId={labelId} />
    {children}
  </button>;
}
