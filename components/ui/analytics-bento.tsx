"use client";

import * as React from "react";

/* ------------------------------------------------------------------ */
/*  Analytics Bento: four live cards (revenue, sessions, categories,   */
/*  seasonality). SVG only, no chart library. Surfaces and text come   */
/*  from the theme (bg-card, text-foreground…); chart colors are props */
/* ------------------------------------------------------------------ */

type Point = { label: string; value: number };

export type AnalyticsBentoProps = React.JSX.IntrinsicElements["div"] & {
  currency?: string;
  revenue: {
    label: string;
    value: number;
    delta: string;
    note?: React.ReactNode;
    caption?: string;
    series: Point[];
  };
  sessions: {
    label: string;
    value: number;
    delta: string;
    note?: React.ReactNode;
    /** values per hour; first array is the current period, the rest are prior periods */
    desktop: number[][];
    mobile: number[][];
    labels?: { desktop: string; mobile: string; prior: string; current: string };
  };
  categories: {
    title: string;
    groups: { id: string; name: string; color: string; ink?: "dark" | "light" }[];
    periods: { label: string; total: number; delta: string; compare: string }[];
    /** one value per period, in the same order as `periods` */
    items: { name: string; group: string; values: number[] }[];
    table?: { columns: string[]; rows: { label: string; values: string[] }[] };
  };
  seasonality: {
    title: string;
    axis?: string[];
    years: {
      label: string;
      values: number[];
      stats: { label: string; value: number; delta: string; note: string }[];
      rows: { label: string; value: string }[];
    }[];
  };
  /** Chart colors. Each can be any CSS color. */
  colors?: { revenue?: [string, string]; desktop?: [string, string]; mobile?: [string, string]; radar?: string };
  fontFamily?: string;
  monoFamily?: string;
  /** The "by vasnev.art" badge under the grid. */
  credit?: boolean;
};

/* ---------------- helpers ---------------- */

const fmt = (n: number) => Math.round(n).toLocaleString("ru-RU");
const ease = (t: number) => 1 - Math.pow(1 - t, 3);

function useReducedMotion() {
  const [r, setR] = React.useState(false);
  React.useEffect(() => {
    const m = window.matchMedia("(prefers-reduced-motion: reduce)");
    setR(m.matches);
    const on = () => setR(m.matches);
    m.addEventListener("change", on);
    return () => m.removeEventListener("change", on);
  }, []);
  return r;
}

function useSize() {
  const ref = React.useRef(null as HTMLDivElement | null);
  const [size, setSize] = React.useState({ w: 0, h: 0 });
  React.useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver(([e]) => {
      const w = Math.round(e.contentRect.width), h = Math.round(e.contentRect.height);
      setSize((p) => (p.w === w && p.h === h ? p : { w, h }));
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  return [ref, size] as const;
}

/** smooth path through points (catmull-rom as cubic beziers) */
function smooth(P: [number, number][]) {
  const n = P.length;
  if (n < 2) return "";
  const g = (i: number) => P[Math.max(0, Math.min(n - 1, i))];
  let d = `M${P[0][0]},${P[0][1]}`;
  for (let i = 0; i < n - 1; i++) {
    const p0 = g(i - 1), p1 = g(i), p2 = g(i + 1), p3 = g(i + 2), k = 0.18;
    d += `C${p1[0] + (p2[0] - p0[0]) * k},${p1[1] + (p2[1] - p0[1]) * k} ${p2[0] - (p3[0] - p1[0]) * k},${p2[1] - (p3[1] - p1[1]) * k} ${p2[0]},${p2[1]}`;
  }
  return d;
}

function CountUp({ value, prefix = "", duration = 1100 }: { value: number; prefix?: string; duration?: number }) {
  const ref = React.useRef(null as HTMLSpanElement | null);
  const from = React.useRef(0);
  const reduce = useReducedMotion();
  React.useEffect(() => {
    const el = ref.current;
    if (!el) return;
    if (reduce) { el.textContent = prefix + fmt(value); from.current = value; return; }
    const a = from.current, t0 = performance.now();
    let raf = 0;
    const f = (now: number) => {
      const t = Math.min(1, (now - t0) / duration), v = a + (value - a) * ease(t);
      el.textContent = prefix + fmt(v); from.current = v;
      if (t < 1) raf = requestAnimationFrame(f);
    };
    raf = requestAnimationFrame(f);
    return () => cancelAnimationFrame(raf);
  }, [value, prefix, duration, reduce]);
  return <span ref={ref}>{prefix}0</span>;
}

/* ---------------- tooltip ---------------- */

type Tip = { title: string; rows: { label: string; value: string; color?: string }[] };
type TipApi = { show: (x: number, y: number, t: Tip) => void; hide: () => void };
type TipRef = { current: TipApi | null };

function Tooltip({ mono, api }: { mono: string; api: TipRef }) {
  const [tip, setTip] = React.useState(null as (Tip & { x: number; y: number }) | null);
  const [pos, setPos] = React.useState({ l: 0, t: 0 });
  const box = React.useRef(null as HTMLDivElement | null);
  React.useEffect(() => {
    api.current = { show: (x, y, t) => setTip({ ...t, x, y }), hide: () => setTip(null) };
    return () => { api.current = null; };
  }, [api]);
  React.useLayoutEffect(() => {
    if (!tip || !box.current) return;
    const r = box.current.getBoundingClientRect();
    let l = tip.x + 14, t = tip.y - r.height - 12;
    if (l + r.width > window.innerWidth - 8) l = tip.x - r.width - 14;
    if (t < 8) t = tip.y + 16;
    setPos({ l, t });
  }, [tip]);
  return (
    <div
      ref={box}
      role="tooltip"
      className={`pointer-events-none fixed z-50 whitespace-nowrap rounded-xl border border-border bg-popover/95 px-2.5 py-2 text-xs text-popover-foreground shadow-lg backdrop-blur-md transition-opacity duration-150 ${tip ? "opacity-100" : "opacity-0"}`}
      style={{ left: pos.l, top: pos.t }}
    >
      {tip && (
        <>
          <div className="mb-1 text-muted-foreground">{tip.title}</div>
          {tip.rows.map((r) => (
            <div key={r.label} className="flex items-center justify-between gap-3">
              <span className="inline-flex items-center gap-1.5 text-muted-foreground">
                {r.color && <i className="inline-block size-2 rounded-[2px]" style={{ background: r.color }} />}
                {r.label}
              </span>
              <b className="font-medium" style={{ fontFamily: mono, fontVariantNumeric: "tabular-nums" }}>{r.value}</b>
            </div>
          ))}
        </>
      )}
    </div>
  );
}

/* ---------------- shared bits ---------------- */

const Arrow = () => (
  <svg viewBox="0 0 12 12" className="size-[11px]" aria-hidden="true">
    <path d="M6 10.5V1.5M2 5.5l4-4 4 4" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" />
  </svg>
);
const Delta = ({ children, className = "" }: { children: React.ReactNode; className?: string }) => (
  <span className={`inline-flex items-center gap-[3px] font-semibold tracking-[-0.02em] text-emerald-600 dark:text-emerald-400 ${className}`}>
    <Arrow />
    {children}
  </span>
);
const Card = ({ className = "", children, ...rest }: React.JSX.IntrinsicElements["section"]) => (
  <section className={`relative min-w-0 overflow-hidden rounded-[28px] border border-border bg-card p-5 text-card-foreground shadow-sm sm:p-6 ${className}`} {...rest}>
    {children}
  </section>
);

function Segmented({ items, value, onChange, label }: { items: string[]; value: number; onChange: (i: number) => void; label: string }) {
  const refs = React.useRef([] as (HTMLButtonElement | null)[]);
  const [thumb, setThumb] = React.useState({ x: 0, w: 0 });
  React.useLayoutEffect(() => {
    const b = refs.current[value];
    if (b) setThumb({ x: b.offsetLeft, w: b.offsetWidth });
  }, [value, items]);
  return (
    <div role="tablist" aria-label={label} className="relative inline-grid h-[30px] auto-cols-fr grid-flow-col rounded-[10px] bg-muted p-[2px]">
      <div
        className="absolute bottom-[2px] top-[2px] rounded-[8px] bg-background shadow-sm ring-1 ring-border transition-[transform,width] duration-300 [transition-timing-function:cubic-bezier(.2,.9,.25,1)]"
        style={{ width: thumb.w, transform: `translateX(${thumb.x - 2}px)`, left: 2 }}
      />
      {items.map((it, i) => (
        <button
          key={it}
          ref={(el) => { refs.current[i] = el; }}
          role="tab"
          type="button"
          aria-selected={i === value}
          onClick={() => onChange(i)}
          className={`relative z-10 rounded-[8px] px-3 text-xs font-medium tracking-[-0.01em] outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring ${i === value ? "text-foreground" : "text-muted-foreground"}`}
        >
          {it}
        </button>
      ))}
    </div>
  );
}

/* ---------------- A: revenue line ---------------- */

function RevenueChart({ series, caption, prefix, colors, mono, tip }: { series: Point[]; caption?: string; prefix: string; colors: [string, string]; mono: string; tip: TipRef }) {
  const [ref, { w: W, h: H }] = useSize();
  const reduce = useReducedMotion();
  const [drawn, setDrawn] = React.useState(false);
  const [hover, setHover] = React.useState(null as number | null);
  const id = React.useId().replace(/:/g, "");
  React.useEffect(() => { if (!W) return; const r = requestAnimationFrame(() => requestAnimationFrame(() => setDrawn(true))); return () => cancelAnimationFrame(r); }, [W]);

  const vals = series.map((s) => s.value);
  const lo = Math.min(...vals), hi = Math.max(...vals), span = hi - lo || 1;
  const raw = span / 4, mag = Math.pow(10, Math.floor(Math.log10(raw))), step = [1, 2, 2.5, 5, 10].map((m) => m * mag).find((s) => s >= raw) || raw;
  const mn = lo - span * 0.45, mx = hi + span * 0.08, top = 30, bot = 14;
  const Y = (v: number) => top + (1 - (v - mn) / (mx - mn)) * (H - top - bot);
  const ticks: number[] = [];
  for (let v = Math.ceil(lo / step) * step; v <= hi + 1e-9; v += step) ticks.push(v);
  const P = vals.map((v, i) => [14 + (i / (vals.length - 1)) * (W - 66), Y(v)] as [number, number]);
  const d = W ? smooth(P) : "";
  const last = P[P.length - 1];
  const compact = (v: number) => (v >= 1000 ? (v / 1000).toFixed(step < 1000 ? 1 : 0) + "K" : String(v));

  const move = (e: React.PointerEvent) => {
    const r = (e.currentTarget as SVGRectElement).ownerSVGElement!.getBoundingClientRect();
    const i = Math.max(0, Math.min(vals.length - 1, Math.round(((e.clientX - r.left - 14) / (W - 66)) * (vals.length - 1))));
    setHover(i);
    tip.current?.show(r.left + P[i][0], r.top + P[i][1], { title: series[i].label, rows: [{ label: "Скачивания", value: prefix + fmt(vals[i]), color: colors[1] }] });
  };

  return (
    <div ref={ref} className="relative min-h-[130px] flex-1 overflow-hidden rounded-[18px] border border-border bg-muted/40 dark:bg-muted/30">
      {caption && <div className="pointer-events-none absolute left-3 top-2.5 text-[11px] tracking-[-0.01em] text-muted-foreground">{caption}</div>}
      {W > 0 && (
        <svg width={W} height={H} className="block" role="img" aria-label={`${caption ?? "Revenue"}: from ${prefix}${fmt(vals[0])} to ${prefix}${fmt(vals[vals.length - 1])}`}>
          <defs>
            <linearGradient id={`${id}l`} x1="0" x2="1" y1="0" y2="0">
              <stop offset="0" style={{ stopColor: colors[0] }} />
              <stop offset="1" style={{ stopColor: colors[1] }} />
            </linearGradient>
            <linearGradient id={`${id}a`} x1="0" x2="0" y1="0" y2="1">
              <stop offset="0" style={{ stopColor: colors[1], stopOpacity: 0.18 }} />
              <stop offset="1" style={{ stopColor: colors[1], stopOpacity: 0 }} />
            </linearGradient>
            <linearGradient id={`${id}m`} x1="0" x2="1" y1="0" y2="0">
              <stop offset="0" stopColor="#fff" stopOpacity="0.2" />
              <stop offset="1" stopColor="#fff" stopOpacity="1" />
            </linearGradient>
            <mask id={`${id}k`}><rect width={W} height={H} fill={`url(#${id}m)`} /></mask>
          </defs>
          {ticks.map((v) => (
            <g key={v}>
              <line x1={0} x2={W} y1={Y(v)} y2={Y(v)} className="stroke-border" strokeWidth={1} />
              <text x={W - 10} y={Y(v) - 5} textAnchor="end" fontSize={10} className="fill-muted-foreground" style={{ fontFamily: mono }}>{compact(v)}</text>
            </g>
          ))}
          <path d={`${d}L${last[0]},${H}L${P[0][0]},${H}Z`} fill={`url(#${id}a)`} mask={`url(#${id}k)`} />
          <path
            d={d}
            pathLength={1}
            fill="none"
            stroke={`url(#${id}l)`}
            strokeWidth={2.5}
            strokeLinecap="round"
            style={{ strokeDasharray: 1, strokeDashoffset: drawn || reduce ? 0 : 1, transition: reduce ? "none" : "stroke-dashoffset 1.4s cubic-bezier(.3,.8,.2,1) .2s" }}
          />
          <circle cx={last[0]} cy={last[1]} r={4.5} style={{ fill: colors[1] }} className="stroke-card" strokeWidth={2} />
          {hover !== null && (
            <>
              <line x1={P[hover][0]} x2={P[hover][0]} y1={0} y2={H} className="stroke-muted-foreground/50" strokeWidth={1} />
              <circle cx={P[hover][0]} cy={P[hover][1]} r={4.5} style={{ fill: colors[1] }} className="stroke-card" strokeWidth={2} />
            </>
          )}
          <rect width={W} height={H} fill="transparent" onPointerMove={move} onPointerLeave={() => { setHover(null); tip.current?.hide(); }} />
        </svg>
      )}
    </div>
  );
}

/* ---------------- B: desktop vs mobile waves ---------------- */

function SessionsChart({ desktop, mobile, labels, colors, tip }: { desktop: number[][]; mobile: number[][]; labels: { desktop: string; mobile: string; current: string }; colors: { desktop: [string, string]; mobile: [string, string] }; tip: TipRef }) {
  const [ref, { w: W, h: H }] = useSize();
  const reduce = useReducedMotion();
  const id = React.useId().replace(/:/g, "");
  const paths = React.useRef([] as (SVGPathElement | null)[]);
  const [hover, setHover] = React.useState(null as number | null);
  const n = desktop[0]?.length ?? 0;
  const max = Math.max(1, ...desktop.flat(), ...mobile.flat());
  const pad = 18, mid = H / 2, amp = H / 2 - 14;
  const X = (i: number) => pad + 6 + (i / (n - 1)) * (W - 2 * pad - 12);
  const sets = React.useMemo(
    () => [
      ...desktop.map((a, k) => ({ a, dir: -1, k, g: "d" })),
      ...mobile.map((a, k) => ({ a, dir: 1, k, g: "m" })),
    ],
    [desktop, mobile],
  );

  React.useEffect(() => {
    if (!W || !H) return;
    let raf = 0;
    const t0 = performance.now();
    const frame = (now: number) => {
      const t = (now - t0) / 1000, grow = reduce ? 1 : ease(Math.min(1, t / 1.3));
      sets.forEach((s, j) => {
        const breathe = reduce ? 1 : 1 + Math.sin(t * 1.2 + s.k * 0.9) * 0.035;
        const P = s.a.map((v, i) => [X(i), mid + s.dir * (3 + (v / max) * amp * grow * breathe)] as [number, number]);
        paths.current[j]?.setAttribute("d", smooth(P));
      });
      if (!reduce) raf = requestAnimationFrame(frame);
    };
    raf = requestAnimationFrame(frame);
    return () => cancelAnimationFrame(raf);
  }, [W, H, sets, reduce, max]);

  const move = (e: React.PointerEvent) => {
    const r = (e.currentTarget as SVGRectElement).ownerSVGElement!.getBoundingClientRect();
    const i = Math.max(0, Math.min(n - 1, Math.round(((e.clientX - r.left - pad - 6) / (W - 2 * pad - 12)) * (n - 1))));
    setHover(i);
    tip.current?.show(r.left + X(i), r.top + 12, {
      title: `${String(i % 24).padStart(2, "0")}:00 · ${labels.current}`,
      rows: [
        { label: labels.desktop, value: fmt(desktop[0][i]), color: colors.desktop[0] },
        { label: labels.mobile, value: fmt(mobile[0][i]), color: colors.mobile[1] },
      ],
    });
  };

  return (
    <div ref={ref} className="relative min-h-[140px] flex-1 overflow-hidden rounded-[18px] border border-border bg-muted/40 dark:bg-muted/30">
      {W > 0 && (
        <svg width={W} height={H} className="block" role="img" aria-label={`${labels.desktop} and ${labels.mobile} by hour`}>
          <defs>
            <linearGradient id={`${id}d`} x1="0" x2="1" y1="0" y2="0">
              <stop offset=".1" style={{ stopColor: colors.desktop[0] }} />
              <stop offset=".9" style={{ stopColor: colors.desktop[1] }} />
            </linearGradient>
            <linearGradient id={`${id}m`} x1="0" x2="1" y1="0" y2="0">
              <stop offset=".15" style={{ stopColor: colors.mobile[0] }} />
              <stop offset=".85" style={{ stopColor: colors.mobile[1] }} />
            </linearGradient>
          </defs>
          {Array.from({ length: 9 }, (_, i) => 10 + ((H - 20) * i) / 8).map((y) => (
            <g key={y} className="stroke-muted-foreground/50">
              <line x1={8} x2={14} y1={y} y2={y} strokeWidth={1} />
              <line x1={W - 14} x2={W - 8} y1={y} y2={y} strokeWidth={1} />
            </g>
          ))}
          <line x1={pad + 6} x2={W - pad - 6} y1={mid} y2={mid} className="stroke-border" strokeWidth={1} />
          {sets.map((s, j) => (
            <path
              key={`${s.g}${s.k}`}
              ref={(el) => { paths.current[j] = el; }}
              fill="none"
              stroke={`url(#${id}${s.g})`}
              strokeWidth={s.k === 0 ? 2.5 : 1.6}
              strokeLinecap="round"
              opacity={s.k === 0 ? 1 : s.k === 1 ? 0.55 : 0.3}
            />
          ))}
          {hover !== null && <line x1={X(hover)} x2={X(hover)} y1={4} y2={H - 4} className="stroke-muted-foreground/60" strokeWidth={1} />}
          <rect width={W} height={H} fill="transparent" onPointerMove={move} onPointerLeave={() => { setHover(null); tip.current?.hide(); }} />
        </svg>
      )}
    </div>
  );
}

/* ---------------- C: treemap ---------------- */

type Rect = { name: string; group: string; v: number; x: number; y: number; w: number; h: number };
function squarify(items: { name: string; group: string; v: number }[], x: number, y: number, w: number, h: number): Rect[] {
  const total = items.reduce((a, b) => a + b.v, 0) || 1, scale = (w * h) / total, out: Rect[] = [];
  let rest = items.map((i) => ({ ...i, a: i.v * scale }));
  const worst = (row: typeof rest, len: number) => {
    const s = row.reduce((a, b) => a + b.a, 0), mx = Math.max(...row.map((r) => r.a)), mn = Math.min(...row.map((r) => r.a));
    return Math.max((len * len * mx) / (s * s), (s * s) / (len * len * mn));
  };
  while (rest.length) {
    const len = Math.min(w, h);
    const row = [rest[0]];
    let i = 1;
    while (i < rest.length && worst([...row, rest[i]], len) <= worst(row, len)) row.push(rest[i++]);
    rest = rest.slice(i);
    const s = row.reduce((a, b) => a + b.a, 0);
    if (w >= h) { const cw = s / h; let cy = y; for (const r of row) { const rh = r.a / cw; out.push({ ...r, x, y: cy, w: cw, h: rh }); cy += rh; } x += cw; w -= cw; }
    else { const rh = s / w; let cx = x; for (const r of row) { const rw = r.a / rh; out.push({ ...r, x: cx, y, w: rw, h: rh }); cy += rh; } y += rh; h -= rh; }
  }
  return out;
}

const TileIcon = () => (
  <svg viewBox="0 0 14 14" className="size-3.5 shrink-0" aria-hidden="true">
    <rect x="1" y="1" width="12" height="12" rx="4" fill="currentColor" opacity=".9" />
    <path d="M7 3.6l1 2.4 2.4 1-2.4 1L7 10.4 6 8 3.6 7 6 6z" fill="var(--tile-bg)" />
  </svg>
);

function Treemap({ data, period, mono, tip }: { data: AnalyticsBentoProps["categories"]; period: number; mono: string; tip: TipRef }) {
  const [ref, { w: W, h: H }] = useSize();
  const [placed, setPlaced] = React.useState(false);
  React.useEffect(() => { if (W) { const r = requestAnimationFrame(() => requestAnimationFrame(() => setPlaced(true))); return () => cancelAnimationFrame(r); } }, [W]);
  const G = 6;
  const items = data.items.map((i) => ({ name: i.name, group: i.group, v: i.values[period] ?? 0 })).sort((a, b) => b.v - a.v);
  const total = items.reduce((a, b) => a + b.v, 0) || 1;
  const rects = W ? squarify(items, 0, 0, W + G, H + G) : [];
  return (
    <div ref={ref} className="relative mt-1.5 h-[300px] sm:h-[340px]" role="img" aria-label={`${data.title}, ${data.periods[period]?.label}`}>
      {rects.map((r) => {
        const g = data.groups.find((x) => x.id === r.group)!;
        const w = r.w - G, h = r.h - G, share = ((r.v / total) * 100).toFixed(2).replace(/\.?0+$/, "") + "%";
        const chars = fmt(r.v).length;
        let fs = Math.min(30, (w - 24 - 58) / (chars * 0.62)), showPct = true;
        if (fs < 17) { showPct = false; fs = Math.min(30, (w - 24) / (chars * 0.62)); }
        const tiny = fs < 12 || h < 54 || w < 60, showName = h >= 78 && w >= 96;
        const ink = g.ink === "light" ? "text-white" : g.ink === "dark" ? "text-neutral-950" : "text-foreground";
        return (
          <div
            key={r.name}
            className={`absolute flex flex-col justify-between overflow-hidden rounded-[14px] p-3 transition-[left,top,width,height,filter] duration-700 [transition-timing-function:cubic-bezier(.2,.9,.25,1)] hover:brightness-[1.08] motion-reduce:transition-none ${ink}`}
            style={{
              left: placed ? r.x : W / 2, top: placed ? r.y : H / 2, width: placed ? w : 0, height: placed ? h : 0,
              background: g.color, ["--tile-bg" as string]: g.color,
            }}
            onPointerMove={(e) => tip.current?.show(e.clientX, e.clientY, { title: `${data.periods[period]?.label} · ${g.name}`, rows: [{ label: r.name, value: fmt(r.v), color: g.color }, { label: "Доля", value: share }] })}
            onPointerLeave={() => tip.current?.hide()}
          >
            <div className="flex items-center gap-1.5 text-[10.5px] font-semibold uppercase tracking-[0.04em]">
              <TileIcon />
              {showName && <span className="truncate">{r.name}</span>}
            </div>
            {!tiny && (
              <div className="flex items-end justify-between gap-1.5">
                <span className="font-medium leading-[.9] tracking-[-0.04em]" style={{ fontFamily: mono, fontSize: fs, fontVariantNumeric: "tabular-nums" }}>
                  <CountUp value={r.v} duration={900} />
                </span>
                {showPct && (
                  <span className="rounded-md bg-black/80 px-[5px] py-[3px] text-[10px] font-medium tracking-[-0.02em] text-white" style={{ fontFamily: mono }}>{share}</span>
                )}
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}

/* ---------------- D: seasonality radar ---------------- */

function Radar({ values, axis, year, color, prefix, mono, tip }: { values: number[]; axis: string[]; year: string; color: string; prefix: string; mono: string; tip: TipRef }) {
  const [ref, { w }] = useSize();
  const reduce = useReducedMotion();
  const line = React.useRef(null as SVGPathElement | null);
  const cur = React.useRef(null as number[] | null);
  const [hover, setHover] = React.useState(null as number | null);
  const N = values.length, S = Math.min(w, 420), R = S / 2 - 34, C = S / 2;
  const ang = (i: number) => -Math.PI / 2 + (i / N) * Math.PI * 2;

  const norm = (v: number[]) => { const lo = Math.min(...v), hi = Math.max(...v); return v.map((x) => 0.42 + ((x - lo) / (hi - lo || 1)) * 0.58); };
  const fit = (v: number[]) => {
    const K = Math.min(5, Math.floor(N / 2)), co: [number, number][] = [];
    for (let k = 0; k <= K; k++) {
      let a = 0, b = 0;
      v.forEach((x, i) => { const th = (i / N) * Math.PI * 2; a += x * Math.cos(k * th); b += x * Math.sin(k * th); });
      co.push([(a * (k ? 2 : 1)) / N, (b * (k ? 2 : 1)) / N]);
    }
    return (th: number) => co.reduce((r, [a, b], k) => r + a * Math.cos(k * th) + b * Math.sin(k * th), 0);
  };
  const at = (f: (t: number) => number, th: number): [number, number] => {
    const v = Math.max(0.05, f(th)), a = -Math.PI / 2 + th;
    return [C + Math.cos(a) * (R * 0.2 + v * R * 0.8), C + Math.sin(a) * (R * 0.2 + v * R * 0.8)];
  };
  const shape = (v: number[]) => { const f = fit(norm(v)); let d = ""; for (let j = 0; j <= 180; j++) { const p = at(f, (j / 180) * Math.PI * 2); d += (j ? "L" : "M") + p[0].toFixed(2) + "," + p[1].toFixed(2); } return d + "Z"; };
  const point = (v: number[], i: number) => at(fit(norm(v)), (i / N) * Math.PI * 2);

  React.useEffect(() => {
    if (!S) return;
    const from = cur.current ?? values.map(() => 1), to = values, t0 = performance.now();
    let raf = 0;
    const step = (now: number) => {
      const t = reduce ? 1 : ease(Math.min(1, (now - t0) / 900));
      cur.current = from.map((v, i) => v + (to[i] - v) * t);
      line.current?.setAttribute("d", shape(cur.current));
      if (t < 1) raf = requestAnimationFrame(step);
    };
    raf = requestAnimationFrame(step);
    return () => cancelAnimationFrame(raf);
  }, [values, S, reduce]);

  const move = (e: React.PointerEvent) => {
    const r = (e.currentTarget as SVGRectElement).ownerSVGElement!.getBoundingClientRect(), x = e.clientX - r.left - C, y = e.clientY - r.top - C;
    if (Math.hypot(x, y) < R * 0.2) { setHover(null); tip.current?.hide(); return; }
    let a = Math.atan2(y, x) + Math.PI / 2; if (a < 0) a += Math.PI * 2;
    const i = Math.round((a / (Math.PI * 2)) * N) % N, p = point(values, i);
    setHover(i);
    tip.current?.show(r.left + p[0], r.top + p[1], { title: `${axis[i]} ${year}`, rows: [{ label: "Активность", value: prefix + fmt(values[i]), color }] });
  };
  const hp = hover !== null ? point(values, hover) : null;

  return (
    <div ref={ref} className="mx-auto mt-1.5 w-full max-w-[420px]" style={{ height: S || 300 }}>
      {S > 0 && (
        <svg width={S} height={S} className="mx-auto block" role="img" aria-label={`Activity by month, ${year}`}>
          {[0.33, 0.66, 1].map((k) => <circle key={k} cx={C} cy={C} r={R * k} fill="none" className="stroke-border" strokeWidth={1} />)}
          {axis.map((m, i) => (
            <g key={m}>
              <line x1={C + Math.cos(ang(i)) * R * 0.2} y1={C + Math.sin(ang(i)) * R * 0.2} x2={C + Math.cos(ang(i)) * R} y2={C + Math.sin(ang(i)) * R} className="stroke-border" strokeWidth={1} />
              <text x={C + Math.cos(ang(i)) * (R + 18)} y={C + Math.sin(ang(i)) * (R + 18)} textAnchor="middle" dominantBaseline="central" fontSize={11} className="fill-muted-foreground" style={{ fontFamily: mono }}>{m}</text>
            </g>
          ))}
          <path ref={line} fill="none" strokeWidth={2.5} strokeLinejoin="round" style={{ stroke: color }} />
          <circle cx={C} cy={C} r={R * 0.2} className="fill-card stroke-border" strokeWidth={1} />
          <g transform={`translate(${C - 11} ${C - 11})`} fill="none" className="stroke-muted-foreground" strokeWidth={1.4} strokeLinejoin="round">
            <path d="M11 1.5l8.5 5v9L11 20.5l-8.5-5v-9z M11 1.5v19 M2.5 6.5L19.5 15.5 M19.5 6.5L2.5 15.5" />
          </g>
          {hp && <circle cx={hp[0]} cy={hp[1]} r={4.5} className="stroke-card" strokeWidth={2} style={{ fill: color }} />}
          <rect width={S} height={S} fill="transparent" onPointerMove={move} onPointerLeave={() => { setHover(null); tip.current?.hide(); }} />
        </svg>
      )}
    </div>
  );
}

/* ---------------- the bento ---------------- */

export function AnalyticsBento({
  currency = "",
  revenue,
  sessions,
  categories,
  seasonality,
  colors = {},
  fontFamily = "Inter, Geist, ui-sans-serif, system-ui, -apple-system, sans-serif",
  monoFamily = "'JetBrains Mono', 'Geist Mono', ui-monospace, SFMono-Regular, monospace",
  credit = false,
  className = "",
  style,
  ...rest
}: AnalyticsBentoProps) {
  const tip = React.useRef(null as TipApi | null) as TipRef;
  const [period, setPeriod] = React.useState(0);
  const [year, setYear] = React.useState(0);
  const [shown, setShown] = React.useState(false);
  React.useEffect(() => { const r = requestAnimationFrame(() => setShown(true)); return () => cancelAnimationFrame(r); }, []);

  const c = {
    revenue: colors.revenue ?? ["#38bdf8", "#22d3ee"],
    desktop: colors.desktop ?? ["#38bdf8", "#818cf8"],
    mobile: colors.mobile ?? ["#a78bfa", "#f472b6"],
    radar: colors.radar ?? "#38bdf8",
  } as const;
  const mono: React.CSSProperties = { fontFamily: monoFamily, fontVariantNumeric: "tabular-nums", letterSpacing: "-0.04em" };
  const Y = seasonality.years[year];
  const sl = { desktop: "Windows", mobile: "macOS/Linux", prior: "Прошлая неделя", current: "Эта неделя", ...sessions.labels };
  const enter = (i: number) =>
    `transition-[opacity,transform] duration-1000 [transition-timing-function:cubic-bezier(.2,.9,.25,1)] motion-reduce:transition-none ${shown ? "opacity-100 translate-y-0" : "opacity-0 translate-y-3.5"} ` +
    ["", "delay-75", "delay-150", "delay-200"][i];

  return (
    <div
      className={`ab-root w-full text-foreground ${className}`}
      style={{ fontFamily, ...style }}
      {...rest}
    >
      <div className="grid grid-cols-12 gap-4">
        {/* revenue / downloads */}
        <Card className={`col-span-12 flex flex-col lg:col-span-5 ${enter(0)}`} aria-label={revenue.label}>
          <div className="flex flex-1 flex-col gap-5 sm:flex-row">
            <div className="shrink-0">
              <div className="text-[13px] tracking-[-0.01em] text-muted-foreground">{revenue.label}</div>
              <div className="mt-2.5 text-[38px] font-medium leading-none sm:text-[44px]" style={mono}>
                <CountUp value={revenue.value} prefix={currency} />
                <Delta className="ml-2.5 align-[6px] text-sm">{revenue.delta}</Delta>
              </div>
              {revenue.note && <div className="mt-2 text-[13px] tracking-[-0.01em] text-muted-foreground/80">{revenue.note}</div>}
            </div>
            <RevenueChart series={revenue.series} caption={revenue.caption} prefix={currency} colors={c.revenue as [string, string]} mono={monoFamily} tip={tip} />
          </div>
        </Card>

        {/* sessions */}
        <Card className={`col-span-12 flex flex-col lg:col-span-7 ${enter(1)}`} aria-label={sessions.label}>
          <div className="flex flex-1 flex-col gap-5 sm:flex-row">
            <div className="shrink-0">
              <div className="text-[13px] tracking-[-0.01em] text-muted-foreground">{sessions.label}</div>
              <div className="mt-2.5 text-[38px] font-medium leading-none sm:text-[44px]" style={mono}>
                <CountUp value={sessions.value} />
                <Delta className="ml-2.5 align-[6px] text-sm">{sessions.delta}</Delta>
              </div>
              {sessions.note && <div className="mt-2 text-[13px] tracking-[-0.01em] text-muted-foreground/80">{sessions.note}</div>}
              <div className="mt-3.5 flex flex-wrap gap-x-3.5 gap-y-1.5 text-xs tracking-[-0.01em] text-muted-foreground">
                <span className="inline-flex items-center gap-1.5"><i className="h-0.5 w-3.5 rounded-sm" style={{ background: `linear-gradient(90deg,${c.desktop[0]},${c.desktop[1]})` }} />{sl.desktop}</span>
                <span className="inline-flex items-center gap-1.5"><i className="h-0.5 w-3.5 rounded-sm" style={{ background: `linear-gradient(90deg,${c.mobile[0]},${c.mobile[1]})` }} />{sl.mobile}</span>
                {(sessions.desktop.length > 1 || sessions.mobile.length > 1) && <span className="inline-flex items-center gap-1.5"><i className="h-0.5 w-3.5 rounded-sm bg-muted-foreground/50" />{sl.prior}</span>}
              </div>
            </div>
            <SessionsChart desktop={sessions.desktop} mobile={sessions.mobile} labels={sl} colors={{ desktop: [...c.desktop] as [string, string], mobile: [...c.mobile] as [string, string] }} tip={tip} />
          </div>
        </Card>

        {/* categories */}
        <Card className={`col-span-12 lg:col-span-7 ${enter(2)}`} aria-label={categories.title}>
          <div className="flex flex-col items-start justify-between gap-3 sm:flex-row sm:items-center">
            <h2 className="m-0 text-[17px] font-semibold tracking-[-0.02em]">{categories.title}</h2>
            <div className="flex flex-wrap gap-x-3.5 gap-y-1.5 text-xs tracking-[-0.01em] text-muted-foreground">
              {categories.groups.map((g) => (
                <span key={g.id} className="inline-flex items-center gap-1.5"><i className="size-2 rounded-[2px]" style={{ background: g.color }} />{g.name}</span>
              ))}
            </div>
          </div>
          <div className="my-[18px] grid grid-cols-3 gap-1.5 sm:gap-3">
            {categories.periods.map((p, i) => (
              <button
                key={p.label}
                type="button"
                aria-pressed={i === period}
                onClick={() => setPeriod(i)}
                className="-m-2.5 rounded-[14px] p-2.5 text-left outline-none transition-colors hover:bg-muted/60 focus-visible:ring-2 focus-visible:ring-ring"
              >
                <div className="flex items-center gap-1.5 text-[13px] font-semibold tracking-[-0.01em]">
                  <i className={`size-1.5 rounded-full transition-colors ${i === period ? "bg-cyan-500 dark:bg-cyan-400" : "bg-muted-foreground/60"}`} />
                  {p.label}
                </div>
                <div className="mt-1.5 text-xl font-medium leading-none sm:text-[28px]" style={mono}>
                  <CountUp value={p.total} prefix={currency} />
                  <Delta className="ml-1.5 hidden align-[4px] text-xs sm:inline-flex">{p.delta}</Delta>
                </div>
                <div className="mt-1.5 hidden text-xs tracking-[-0.01em] text-muted-foreground/80 sm:block">{p.compare}</div>
              </button>
            ))}
          </div>
          <Treemap data={categories} period={period} mono={monoFamily} tip={tip} />
          {categories.table && (
            <table className="mt-[18px] w-full border-collapse text-sm">
              <thead>
                <tr>{categories.table.columns.map((h, i) => <th key={h} className={`pb-2 text-xs font-medium tracking-[-0.01em] text-muted-foreground ${i ? "text-right" : "text-left"}`}>{h}</th>)}</tr>
              </thead>
              <tbody>
                {categories.table.rows.map((r) => (
                  <tr key={r.label} className="border-t border-border">
                    <td className="py-[11px] text-left tracking-[-0.01em] text-muted-foreground">{r.label}</td>
                    {r.values.map((v, i) => <td key={i} className="py-[11px] text-right" style={mono}>{v}</td>)}
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </Card>

        {/* seasonality */}
        <Card className={`col-span-12 lg:col-span-5 ${enter(3)}`} aria-label={seasonality.title}>
          <div className="flex flex-col items-start justify-between gap-3 sm:flex-row sm:items-center">
            <h2 className="m-0 text-[17px] font-semibold tracking-[-0.02em]">{seasonality.title}</h2>
            <Segmented label="Год" items={seasonality.years.map((y) => y.label)} value={year} onChange={setYear} />
          </div>
          <Radar
            values={Y.values}
            axis={seasonality.axis ?? ["Янв", "Фев", "Мар", "Апр", "Май", "Июн", "Июл", "Авг", "Сен", "Окт", "Ноя", "Дек"]}
            year={Y.label}
            color={c.radar}
            prefix={currency}
            mono={monoFamily}
            tip={tip}
          />
          <div className="mt-2 grid grid-cols-2 gap-3">
            {Y.stats.map((s) => (
              <div key={s.label}>
                <div className="text-[13px] tracking-[-0.01em] text-muted-foreground">{s.label}</div>
                <div className="mt-1.5 text-[26px] font-medium leading-none sm:text-[30px]" style={mono}><CountUp value={s.value} prefix={currency} /></div>
                <div className="mt-1.5 text-xs text-muted-foreground/80"><Delta className="mr-1.5">{s.delta}</Delta><span style={mono}>{s.note}</span></div>
              </div>
            ))}
          </div>
          <div className="mt-3.5">
            {Y.rows.map((r) => (
              <div key={r.label} className="flex justify-between border-t border-border py-[11px] text-sm">
                <span className="tracking-[-0.01em] text-muted-foreground">{r.label}</span>
                <span style={mono}>{r.value}</span>
              </div>
            ))}
          </div>
        </Card>
      </div>

      <Tooltip api={tip} mono={monoFamily} />
    </div>
  );
}

export default AnalyticsBento;
