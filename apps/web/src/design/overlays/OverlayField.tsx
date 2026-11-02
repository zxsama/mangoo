import {
  useId,
  type InputHTMLAttributes,
  type ReactNode,
  type Ref
} from "react";

export { OverlaySelect, type OverlaySelectProps } from "./OverlaySelect";

export interface OverlayFieldProps extends InputHTMLAttributes<HTMLInputElement> {
  error?: string | null;
  inputRef?: Ref<HTMLInputElement>;
  label?: string;
  leadingIcon?: ReactNode;
  trailingAction?: ReactNode;
}

export function OverlayField({
  "aria-describedby": ariaDescribedBy,
  className,
  disabled,
  error,
  id,
  inputRef,
  label,
  leadingIcon,
  trailingAction,
  ...props
}: OverlayFieldProps) {
  const generatedId = useId();
  const inputId = id ?? `overlay-field-${generatedId}`;
  const errorId = error ? `${inputId}-error` : undefined;
  const describedBy = [ariaDescribedBy, errorId].filter(Boolean).join(" ") || undefined;

  return (
    <div className="overlay-field" data-disabled={disabled || undefined}>
      {label ? <label className="overlay-field-label" htmlFor={inputId}>{label}</label> : null}
      <div className="overlay-field-control">
        {leadingIcon ? (
          <span className="overlay-field-leading" aria-hidden="true">{leadingIcon}</span>
        ) : null}
        <input
          {...props}
          aria-describedby={describedBy}
          aria-invalid={error ? true : props["aria-invalid"]}
          className={`overlay-field-input${className ? ` ${className}` : ""}`}
          data-pointer-glow="off"
          disabled={disabled}
          id={inputId}
          ref={inputRef}
        />
        {trailingAction ? (
          <span className="overlay-field-trailing">{trailingAction}</span>
        ) : null}
      </div>
      {error ? (
        <div className="overlay-field-error" id={errorId} role="alert">{error}</div>
      ) : null}
    </div>
  );
}
