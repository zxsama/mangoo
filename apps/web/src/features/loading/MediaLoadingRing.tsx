import { useEffect, useId, useRef, useState, type ReactNode } from "react";

interface MediaLoadingRingProps {
  className?: string;
  size?: number;
}

export function MediaLoadingRing({ className, size }: MediaLoadingRingProps) {
  const gradientId = `media-loading-gradient-${useId().replace(/[^a-zA-Z0-9_-]/g, "")}`;

  return (
    <svg
      aria-hidden="true"
      className={className ?? "media-loading-ring"}
      height={size}
      style={size === undefined ? undefined : { height: size, width: size }}
      viewBox="0 0 30 30"
      width={size}
    >
      <defs>
        <linearGradient id={gradientId} x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stopColor="#d8e5e3" />
          <stop offset="0.32" stopColor="#a9c7c0" />
          <stop offset="0.62" stopColor="#6d9f9f" />
          <stop offset="0.84" stopColor="#7d99aa" />
          <stop offset="1" stopColor="#d8e5e3" />
        </linearGradient>
      </defs>
      <g className="media-loading-ring-flow">
        <circle
          cx="15"
          cy="15"
          fill="none"
          r="12.2"
          stroke={`url(#${gradientId})`}
          strokeWidth="3.45"
        />
      </g>
    </svg>
  );
}

interface MediaLoadingIndicatorProps {
  active: boolean;
  as?: "div" | "span";
  children: ReactNode;
  className: string;
  label: string;
}

const MEDIA_LOADING_FADE_MS = 180;
const MEDIA_LOADING_SETTLE_DELAY_MS = 100;

export function MediaLoadingIndicator({
  active,
  as = "div",
  children,
  className,
  label
}: MediaLoadingIndicatorProps) {
  const [mounted, setMounted] = useState(active);
  const [visible, setVisible] = useState(false);
  const hasAppearedRef = useRef(false);
  const fadeInFrameRef = useRef<number | null>(null);
  const fadeOutDelayRef = useRef<number | null>(null);
  const unmountTimerRef = useRef<number | null>(null);

  useEffect(() => {
    if (fadeInFrameRef.current !== null) {
      window.cancelAnimationFrame(fadeInFrameRef.current);
      fadeInFrameRef.current = null;
    }
    if (fadeOutDelayRef.current !== null) {
      window.clearTimeout(fadeOutDelayRef.current);
      fadeOutDelayRef.current = null;
    }
    if (unmountTimerRef.current !== null) {
      window.clearTimeout(unmountTimerRef.current);
      unmountTimerRef.current = null;
    }

    if (active) {
      setMounted(true);
      if (hasAppearedRef.current) {
        setVisible(true);
      } else {
        setVisible(false);
        fadeInFrameRef.current = window.requestAnimationFrame(() => {
          fadeInFrameRef.current = null;
          hasAppearedRef.current = true;
          setVisible(true);
        });
      }
    } else {
      fadeOutDelayRef.current = window.setTimeout(() => {
        fadeOutDelayRef.current = null;
        setVisible(false);
        unmountTimerRef.current = window.setTimeout(() => {
          unmountTimerRef.current = null;
          hasAppearedRef.current = false;
          setMounted(false);
        }, MEDIA_LOADING_FADE_MS);
      }, MEDIA_LOADING_SETTLE_DELAY_MS);
    }

    return () => {
      if (fadeInFrameRef.current !== null) {
        window.cancelAnimationFrame(fadeInFrameRef.current);
        fadeInFrameRef.current = null;
      }
      if (fadeOutDelayRef.current !== null) {
        window.clearTimeout(fadeOutDelayRef.current);
        fadeOutDelayRef.current = null;
      }
      if (unmountTimerRef.current !== null) {
        window.clearTimeout(unmountTimerRef.current);
        unmountTimerRef.current = null;
      }
    };
  }, [active]);

  if (!mounted) return null;
  const Element = as;
  return (
    <Element
      aria-label={label}
      className={`media-loading-indicator ${className}`}
      data-media-loading-state={visible ? "visible" : "hidden"}
      role="status"
    >
      {children}
    </Element>
  );
}
