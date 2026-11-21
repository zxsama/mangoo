import type { ComponentPropsWithoutRef } from "react";

type ParameterSliderProps = Pick<ComponentPropsWithoutRef<"input">,
  "aria-describedby" | "aria-label" | "aria-valuetext" | "className" | "disabled" | "id" | "max" | "min" | "step"
> & {
  value: number;
  resetValue: number;
  resetLabel: string;
  resetHint?: string;
  onChange(value: number): void;
};

export function ParameterSlider({ value, resetValue, resetLabel, resetHint, onChange, ...props }: ParameterSliderProps) {
  const hint = resetHint ?? `双击滑块或按 Enter 恢复默认值（${resetLabel}）`;
  return (
    <input
      {...props}
      aria-description={hint}
      aria-keyshortcuts="Enter"
      onChange={(event) => onChange(event.currentTarget.valueAsNumber)}
      onDoubleClick={(event) => {
        event.stopPropagation();
        onChange(resetValue);
      }}
      onKeyDown={(event) => {
        if (event.key !== "Enter" || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
        event.preventDefault();
        event.stopPropagation();
        if (!event.repeat) onChange(resetValue);
      }}
      title={hint}
      type="range"
      value={value}
    />
  );
}
