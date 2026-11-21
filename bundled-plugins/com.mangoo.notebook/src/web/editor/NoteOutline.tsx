import { useEffect, useRef, useState, type CSSProperties } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { icons } from "@mangoo/web/plugin-host";
import type { OutlineHeading } from "./outline.worker";

interface Props {
  source: string;
  cursor: number;
  zh: boolean;
  onJump(position: number): void;
  onClose(): void;
}

export function NoteOutline({ source, cursor, zh, onJump, onClose }: Props) {
  const [headings, setHeadings] = useState<OutlineHeading[]>([]);
  const [pending, setPending] = useState(true);
  const [failed, setFailed] = useState(false);
  const worker = useRef<Worker>();
  const running = useRef(false);
  const next = useRef<{ source: string; version: number }>();
  const version = useRef(0);
  const list = useRef<HTMLElement>(null);
  useEffect(() => {
    const instance = new Worker(new URL("./outline.worker.ts", import.meta.url), { type: "module" });
    worker.current = instance;
    instance.onmessage = ({ data }: MessageEvent<{ version: number; headings: OutlineHeading[] }>) => {
      running.current = false;
      if (next.current) {
        instance.postMessage(next.current);
        next.current = undefined;
        running.current = true;
      }
      if (data.version !== version.current) return;
      setHeadings(data.headings);
      setPending(false);
      setFailed(false);
    };
    instance.onerror = () => { running.current = false; next.current = undefined; setFailed(true); setPending(false); };
    return () => { instance.terminate(); worker.current = undefined; running.current = false; next.current = undefined; };
  }, []);
  useEffect(() => {
    const id = ++version.current;
    setPending(true);
    next.current = undefined;
    const timer = window.setTimeout(() => {
      const request = { source, version: id };
      if (running.current) next.current = request;
      else if (worker.current) { running.current = true; worker.current.postMessage(request); }
    }, 280);
    return () => window.clearTimeout(timer);
  }, [source]);
  const virtual = useVirtualizer({ count: headings.length, getScrollElement: () => list.current,
    estimateSize: () => 30, overscan: 4 });
  let low = 0, high = headings.length;
  while (low < high) {
    const middle = Math.floor((low + high) / 2);
    if (headings[middle].from <= cursor) low = middle + 1;
    else high = middle;
  }
  const label = zh ? "文档大纲" : "Document outline";
  return <aside className="nb-outline" aria-label={label} style={{ "--nb-outline-rows": Math.max(1, headings.length) } as CSSProperties}
    onKeyDown={e => { if (e.key === "Escape") { e.stopPropagation(); onClose(); } }}>
    <header><span>{label}</span>
      <button type="button" className="nb-tool" aria-label={zh ? "收起大纲" : "Close outline"}
        title={zh ? "收起大纲" : "Close outline"} onClick={onClose}><icons.X size={13} /></button>
    </header>
    {!headings.length && <p role="status">{failed ? (zh ? "无法读取大纲" : "Outline unavailable")
      : pending ? (zh ? "正在读取标题…" : "Reading headings…") : (zh ? "在正文中添加标题以导航" : "Add a heading to navigate here")}</p>}
    <nav ref={list} aria-label={zh ? "章节" : "Headings"} aria-busy={pending}>
      <div style={{ height: virtual.getTotalSize(), position: "relative" }}>
        {virtual.getVirtualItems().map(row => {
          const heading = headings[row.index];
          return <button type="button" key={heading.from} disabled={pending} title={heading.text} data-heading={row.index}
            aria-current={row.index === low - 1 ? "location" : undefined}
            style={{ "--nb-indent": Math.max(0, heading.level - 1), transform: `translateY(${row.start}px)` } as CSSProperties}
            onKeyDown={event => {
              const index = ({ ArrowDown: row.index + 1, ArrowUp: row.index - 1, Home: 0, End: headings.length - 1 } as Record<string, number>)[event.key];
              if (index === undefined) return;
              event.preventDefault();
              const destination = Math.max(0, Math.min(headings.length - 1, index));
              virtual.scrollToIndex(destination, { align: "auto" });
              requestAnimationFrame(() => list.current?.querySelector<HTMLButtonElement>(`[data-heading="${destination}"]`)?.focus());
            }}
            onClick={() => onJump(heading.from)}><span data-button-content>{heading.text}</span></button>;
        })}
      </div>
    </nav>
  </aside>;
}
