/// Returns the complete dashboard HTML.
/// The `DATA_PLACEHOLDER` string in the HTML is replaced with actual JSON at serve time.
pub fn dashboard_html(builds_json: &str, meta_json: &str) -> String {
    DASHBOARD_TEMPLATE
        .replace("\"__BUILDS_DATA__\"", builds_json)
        .replace("\"__META_DATA__\"", meta_json)
}

const DASHBOARD_TEMPLATE: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1.0" />
  <title>sus ui — nix builds</title>
</head>
<body>
<script type="module">
import { h, render } from "https://cdn.jsdelivr.net/npm/preact@10.25.4/+esm";
import { useState, useEffect, useRef, useMemo, useCallback } from "https://cdn.jsdelivr.net/npm/preact@10.25.4/hooks/+esm";
import htm from "https://cdn.jsdelivr.net/npm/htm@3.1.1/+esm";

const html = htm.bind(h);

// ─── LIVE DATA ─────────────────────────────────────────────
const BUILDS_DATA = "__BUILDS_DATA__";
const META_DATA = "__META_DATA__";

// ─── DESIGN TOKENS ──────────────────────────────────────────
const T = {
  color: {
    void: "#0a0a0f", surface0: "#111118", surface1: "#18181f",
    surface2: "#1f1f28", surface3: "#2a2a35",
    border: "#2e2e3a", borderSubtle: "#1e1e28",
    textPrimary: "#e8e4ec", textSecondary: "#8a8698", textTertiary: "#5c586a",
    saku50: "#fff4f2", saku100: "#ffddd8", saku200: "#ffb5ab",
    saku300: "#ff8c7e", saku400: "#e8604f", saku500: "#cc463a", saku600: "#a63228",
    pass400: "#34d97f", pass500: "#22b861",
    fail400: "#ff5c4d", fail500: "#e63b2e",
    pending400: "#ffbf33", pending500: "#e5a200",
    running400: "#4da3ff", running500: "#2b87e6",
    skipped400: "#5c586a",
    hint: "#b8a9ff", hintBg: "#b8a9ff12", hintBorder: "#b8a9ff25",
  },
  radius: { xs: "4px", sm: "6px", md: "10px", lg: "14px", xl: "20px", full: "9999px" },
  shadow: {
    sm: "0 1px 2px rgba(0,0,0,0.3)", md: "0 4px 12px rgba(0,0,0,0.4)",
    lg: "0 8px 30px rgba(0,0,0,0.5)",
    glow: (c) => `0 0 20px ${c}33, 0 0 60px ${c}11`,
  },
  font: {
    display: "'DM Sans', sans-serif",
    mono: "'JetBrains Mono', 'Fira Code', monospace",
    body: "'DM Sans', sans-serif",
  },
  fontSize: { xs: "11px", sm: "12px", md: "13px", base: "14px", lg: "16px", xl: "20px", "2xl": "28px" },
  transition: {
    fast: "120ms cubic-bezier(0.22, 1, 0.36, 1)",
    base: "200ms cubic-bezier(0.22, 1, 0.36, 1)",
  },
};

const statusConfig = {
  passed:  { color: T.color.pass400,    bg: T.color.pass400 + "18",    label: "Passed",  icon: "✓" },
  failed:  { color: T.color.fail400,    bg: T.color.fail400 + "18",    label: "Failed",  icon: "✕" },
  running: { color: T.color.running400, bg: T.color.running400 + "18", label: "Running", icon: "↻" },
  pending: { color: T.color.pending400, bg: T.color.pending400 + "18", label: "Pending", icon: "◦" },
  skipped: { color: T.color.skipped400, bg: T.color.skipped400 + "18", label: "Skipped", icon: "—" },
};

// ─── CSS ────────────────────────────────────────────────────
const cssText = `
@import url('https://fonts.googleapis.com/css2?family=DM+Sans:ital,opsz,wght@0,9..40,300;0,9..40,400;0,9..40,500;0,9..40,600;0,9..40,700;1,9..40,400&family=JetBrains+Mono:wght@400;500;600&display=swap');
*, *::before, *::after { box-sizing: border-box; margin: 0; padding: 0; }
body {
  background: ${T.color.void}; color: ${T.color.textPrimary};
  font-family: ${T.font.body}; font-size: 14px; line-height: 1.5;
  -webkit-font-smoothing: antialiased;
}
.root {
  min-height: 100vh; background: ${T.color.void};
  position: relative; overflow-x: hidden;
}
.root::before {
  content: ''; position: fixed; inset: 0; opacity: 0.025;
  background-image: url("data:image/svg+xml,%3Csvg viewBox='0 0 256 256' xmlns='http://www.w3.org/2000/svg'%3E%3Cfilter id='n'%3E%3CfeTurbulence type='fractalNoise' baseFrequency='0.9' numOctaves='4' stitchTiles='stitch'/%3E%3C/filter%3E%3Crect width='100%25' height='100%25' filter='url(%23n)'/%3E%3C/svg%3E");
  pointer-events: none; z-index: 9999;
}
.root::after {
  content: ''; position: fixed; top: 0; left: 0; right: 0; height: 2px;
  background: linear-gradient(90deg, transparent 0%, ${T.color.saku400} 30%, ${T.color.saku300} 50%, ${T.color.saku400} 70%, transparent 100%);
  z-index: 100;
}
::-webkit-scrollbar { width: 6px; }
::-webkit-scrollbar-track { background: transparent; }
::-webkit-scrollbar-thumb { background: ${T.color.surface3}; border-radius: 3px; }
@keyframes fadeInUp { from { opacity: 0; transform: translateY(12px); } to { opacity: 1; transform: translateY(0); } }
@keyframes fadeIn { from { opacity: 0; } to { opacity: 1; } }
@keyframes pulse { 0%, 100% { opacity: 1; } 50% { opacity: 0.5; } }
@keyframes spin { from { transform: rotate(0deg); } to { transform: rotate(360deg); } }
@keyframes hintFadeIn { from { opacity: 0; transform: translateY(4px); } to { opacity: 1; transform: translateY(0); } }
.animate-in { animation: fadeInUp 500ms cubic-bezier(0.22, 1, 0.36, 1) forwards; opacity: 0; }
.stagger-1 { animation-delay: 50ms; }
.stagger-2 { animation-delay: 100ms; }
.stagger-3 { animation-delay: 150ms; }
.sus-input::placeholder { color: ${T.color.textTertiary}; }
.gh-link { color: ${T.color.saku300}; text-decoration: none; transition: color 120ms ease; }
.gh-link:hover { color: ${T.color.saku200}; }
.hint-anchor { position: relative; }
.hint-anchor .hint-dot {
  position: absolute; top: -2px; right: -2px; width: 5px; height: 5px;
  border-radius: 50%; background: ${T.color.hint}; opacity: 0;
  transition: opacity 200ms ease; pointer-events: none; z-index: 2;
}
.hint-anchor:hover .hint-dot { opacity: 0.7; }
.hint-anchor .hint-popup {
  display: none; position: absolute; z-index: 1000;
  min-width: 320px; max-width: 440px;
  background: ${T.color.surface0}; border: 1px solid ${T.color.hint}30;
  border-radius: ${T.radius.lg}; padding: 0;
  box-shadow: 0 12px 40px rgba(0,0,0,0.6), 0 0 0 1px ${T.color.hint}10;
  animation: hintFadeIn 180ms cubic-bezier(0.22, 1, 0.36, 1) forwards;
  pointer-events: auto;
}
.hint-anchor:hover .hint-popup { display: block; }
.hint-popup-above { bottom: calc(100% + 8px); left: 0; }
.hint-popup-below { top: calc(100% + 8px); left: 0; }
.hint-popup-right { top: 0; left: calc(100% + 8px); }
.hint-popup-above-right { bottom: calc(100% + 8px); right: 0; left: auto; }
`;

// ─── ICONS ──────────────────────────────────────────────────
function SakuIcon({ size = 28 }) {
  return html`
    <svg width=${size} height=${size} viewBox="0 0 28 28" fill="none">
      <rect x="3" y="8" width="22" height="12" rx="3" fill=${T.color.saku400} opacity="0.15" stroke=${T.color.saku400} stroke-width="1.2"/>
      <line x1="8" y1="9" x2="8" y2="19" stroke=${T.color.saku400} stroke-width="0.8" opacity="0.5" stroke-linecap="round"/>
      <line x1="13" y1="9" x2="13" y2="19" stroke=${T.color.saku400} stroke-width="0.8" opacity="0.5" stroke-linecap="round"/>
      <line x1="18" y1="9" x2="18" y2="19" stroke=${T.color.saku400} stroke-width="0.8" opacity="0.5" stroke-linecap="round"/>
      <rect x="4" y="9" width="20" height="3" rx="1" fill="white" opacity="0.08"/>
    </svg>
  `;
}

function GHIcon({ size = 14 }) {
  return html`
    <svg width=${size} height=${size} viewBox="0 0 16 16" fill="currentColor">
      <path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0016 8c0-4.42-3.58-8-8-8z"/>
    </svg>
  `;
}

// ─── DATA-SOURCE HINT ──────────────────────────────────────
function DataHint({ children, commands, position = "above", files, notes }) {
  const sourceColors = {
    nix:    { bg: "#4da3ff15", color: "#4da3ff", label: "nix" },
    git:    { bg: "#ff8c7e15", color: "#ff8c7e", label: "git" },
    github: { bg: "#e8e4ec15", color: "#e8e4ec", label: "gh" },
    shell:  { bg: "#34d97f15", color: "#34d97f", label: "sh" },
    api:    { bg: "#ffbf3315", color: "#ffbf33", label: "api" },
    fs:     { bg: "#b8a9ff15", color: "#b8a9ff", label: "fs" },
  };
  const posClass = { above: "hint-popup-above", below: "hint-popup-below", right: "hint-popup-right", "above-right": "hint-popup-above-right" }[position] || "hint-popup-above";
  return html`
    <div class="hint-anchor" style=${{ display: "inline-flex" }}>
      <div class="hint-dot" />
      ${children}
      <div class=${"hint-popup " + posClass} onClick=${e => e.stopPropagation()}>
        <div style=${{ padding: "10px 14px 8px", borderBottom: "1px solid " + T.color.hint + "15", display: "flex", alignItems: "center", gap: 6 }}>
          <svg width="12" height="12" viewBox="0 0 16 16" fill=${T.color.hint} opacity="0.8">
            <path d="M8 1a7 7 0 100 14A7 7 0 008 1zm0 2.5a1 1 0 110 2 1 1 0 010-2zM6.5 7h1.25v4.5h1.25v1H6.5v-1h.75V8H6.5V7z"/>
          </svg>
          <span style=${{ fontSize: 10, fontFamily: T.font.mono, fontWeight: 600, color: T.color.hint, textTransform: "uppercase", letterSpacing: "0.08em" }}>Data Sources</span>
        </div>
        ${commands && commands.length > 0 && html`
          <div style=${{ padding: "10px 14px" }}>
            ${commands.map((c, i) => {
              const src = sourceColors[c.source] || sourceColors.shell;
              return html`
                <div key=${i} style=${{ marginBottom: i < commands.length - 1 ? 10 : 0 }}>
                  <div style=${{ display: "flex", alignItems: "center", gap: 6, marginBottom: 4 }}>
                    <span style=${{ fontSize: 9, fontFamily: T.font.mono, fontWeight: 600, padding: "1px 5px", borderRadius: T.radius.xs, background: src.bg, color: src.color, border: "1px solid " + src.color + "25", textTransform: "uppercase", letterSpacing: "0.05em" }}>${src.label}</span>
                    <span style=${{ fontSize: 11, color: T.color.textSecondary, fontWeight: 500 }}>${c.label}</span>
                  </div>
                  <div style=${{ fontFamily: T.font.mono, fontSize: 11, lineHeight: 1.5, color: T.color.textPrimary, background: T.color.surface2, borderRadius: T.radius.sm, padding: "6px 10px", border: "1px solid " + T.color.borderSubtle, whiteSpace: "pre-wrap", wordBreak: "break-all", overflowX: "auto", maxHeight: 120 }}>
                    <span style=${{ color: T.color.textTertiary }}>$ </span>${c.cmd}
                  </div>
                </div>
              `;
            })}
          </div>
        `}
        ${files && files.length > 0 && html`
          <div style=${{ padding: "8px 14px 10px", borderTop: commands && commands.length > 0 ? "1px solid " + T.color.borderSubtle : "none" }}>
            <div style=${{ fontSize: 9, fontFamily: T.font.mono, fontWeight: 600, color: T.color.textTertiary, textTransform: "uppercase", letterSpacing: "0.06em", marginBottom: 6 }}>Files</div>
            ${files.map((f, i) => html`
              <div key=${i} style=${{ display: "flex", alignItems: "baseline", gap: 8, marginBottom: 3 }}>
                <span style=${{ fontFamily: T.font.mono, fontSize: 11, color: T.color.saku300 }}>${f.path}</span>
                ${f.desc && html`<span style=${{ fontSize: 10, color: T.color.textTertiary }}>— ${f.desc}</span>`}
              </div>
            `)}
          </div>
        `}
        ${notes && html`
          <div style=${{ padding: "8px 14px 10px", borderTop: "1px solid " + T.color.borderSubtle, fontSize: 11, color: T.color.textTertiary, lineHeight: 1.5, fontStyle: "italic" }}>${notes}</div>
        `}
      </div>
    </div>
  `;
}

// ─── BASIC COMPONENTS ───────────────────────────────────────
function StatusBadge({ status, size = "md" }) {
  const cfg = statusConfig[status];
  if (!cfg) return html`<span>${status}</span>`;
  const s = size === "sm" ? { px: 8, py: 3, font: 11 } : { px: 10, py: 4, font: 12 };
  return html`
    <span style=${{ display: "inline-flex", alignItems: "center", gap: 5, padding: s.py + "px " + s.px + "px", background: cfg.bg, color: cfg.color, borderRadius: T.radius.full, fontSize: s.font, fontWeight: 500, fontFamily: T.font.body, border: "1px solid " + cfg.color + "25", lineHeight: 1, whiteSpace: "nowrap" }}>
      <span style=${{ width: 6, height: 6, borderRadius: "50%", background: cfg.color, flexShrink: 0, animation: status === "running" ? "pulse 1.5s infinite" : "none" }} />
      ${cfg.label}
    </span>
  `;
}

function SearchInput({ placeholder, value, onChange }) {
  const [focused, setFocused] = useState(false);
  return html`
    <div style=${{ display: "flex", alignItems: "center", gap: 8, padding: "8px 12px", background: T.color.surface1, border: "1px solid " + (focused ? T.color.saku400 + "60" : T.color.border), borderRadius: T.radius.sm, transition: "all " + T.transition.fast, boxShadow: focused ? "0 0 0 3px " + T.color.saku400 + "15" : "none" }}>
      <span style=${{ color: T.color.textTertiary, fontSize: 14, flexShrink: 0 }}>⌕</span>
      <input class="sus-input" placeholder=${placeholder} value=${value}
        onInput=${onChange} onFocus=${() => setFocused(true)} onBlur=${() => setFocused(false)}
        style=${{ background: "transparent", border: "none", outline: "none", color: T.color.textPrimary, fontSize: 13, fontFamily: T.font.body, width: "100%" }} />
    </div>
  `;
}

function MetricCard({ label, value, suffix, sub, color, hintCommands, hintFiles, hintNotes }) {
  const inner = html`
    <div style=${{ background: T.color.surface1, border: "1px solid " + T.color.border, borderRadius: T.radius.lg, padding: "20px", flex: 1, minWidth: 130 }}>
      <div style=${{ fontSize: T.fontSize.sm, color: T.color.textTertiary, marginBottom: 8 }}>${label}</div>
      <div style=${{ display: "flex", alignItems: "baseline", gap: 4 }}>
        <span style=${{ fontSize: T.fontSize["2xl"], fontWeight: 700, color: color || T.color.textPrimary, fontFamily: T.font.display, letterSpacing: "-0.03em" }}>${value}</span>
        ${suffix && html`<span style=${{ fontSize: T.fontSize.sm, color: T.color.textTertiary }}>${suffix}</span>`}
      </div>
      ${sub && html`<div style=${{ fontSize: T.fontSize.xs, color: T.color.textTertiary, marginTop: 4 }}>${sub}</div>`}
    </div>
  `;
  if (hintCommands || hintFiles) {
    return html`<${DataHint} commands=${hintCommands} files=${hintFiles} notes=${hintNotes} position="below">${inner}</${DataHint}>`;
  }
  return inner;
}

function LogLine({ number, content, level }) {
  const colors = { info: T.color.textSecondary, success: T.color.pass400, error: T.color.fail400, warning: T.color.pending400, dim: T.color.textTertiary, nix: T.color.running400 };
  const isErr = level === "error";
  return html`
    <div style=${{ display: "flex", gap: 0, fontFamily: T.font.mono, fontSize: T.fontSize.xs, lineHeight: 1.7, background: isErr ? T.color.fail400 + "08" : "transparent", borderLeft: isErr ? "2px solid " + T.color.fail400 + "40" : "2px solid transparent" }}>
      <span style=${{ color: T.color.textTertiary, width: 44, textAlign: "right", paddingRight: 12, userSelect: "none", flexShrink: 0, opacity: 0.4 }}>${number}</span>
      <span style=${{ color: colors[level] || T.color.textSecondary, whiteSpace: "pre-wrap" }}>${content}</span>
    </div>
  `;
}

function FilterTab({ label, count, active, onClick, color }) {
  const [hov, setHov] = useState(false);
  return html`
    <button onClick=${onClick} onMouseEnter=${() => setHov(true)} onMouseLeave=${() => setHov(false)}
      style=${{ display: "inline-flex", alignItems: "center", gap: 6, padding: "6px 12px", background: active ? (color ? color + "18" : T.color.surface2) : hov ? T.color.surface2 : "transparent", border: active ? "1px solid " + (color ? color + "30" : T.color.border) : "1px solid transparent", borderRadius: T.radius.full, fontSize: T.fontSize.sm, fontWeight: active ? 500 : 400, fontFamily: T.font.body, color: active ? (color || T.color.textPrimary) : T.color.textSecondary, cursor: "pointer", transition: "all " + T.transition.fast, lineHeight: 1 }}>
      ${label}
      ${count !== undefined && html`
        <span style=${{ fontSize: 10, fontFamily: T.font.mono, background: active ? (color ? color + "20" : T.color.surface3) : T.color.surface3, color: active ? (color || T.color.textPrimary) : T.color.textTertiary, padding: "2px 6px", borderRadius: T.radius.full, fontWeight: 600 }}>${count}</span>
      `}
    </button>
  `;
}

// ─── OVERRIDE INPUT PILL ────────────────────────────────────
function OverridePill({ inputName, type, owner, repo, ref: gitRef, pr, flakeRef }) {
  const isGH = type === "github";
  const short = (gitRef && gitRef.length > 12) ? gitRef.slice(0, 7) : gitRef;
  const flakeInputUri = isGH ? "github:" + owner + "/" + repo + "/" + gitRef : gitRef;
  const hintCmds = [
    { label: "Apply this override", cmd: "nix build " + (flakeRef || ".") + " --override-input " + inputName + " " + flakeInputUri, source: "nix" },
    { label: "Inspect original input", cmd: "nix flake metadata " + (flakeRef || ".") + " --json | jq '.locks.nodes.\"" + inputName + "\"'", source: "nix" },
  ];
  if (isGH) hintCmds.push({ label: "Fetch commit info", cmd: "gh api repos/" + owner + "/" + repo + "/commits/" + (gitRef || "HEAD") + " --jq '.sha, .commit.message'", source: "github" });
  if (pr) hintCmds.push({ label: "PR details", cmd: "gh pr view " + pr + " --repo " + owner + "/" + repo + " --json title,state,mergeCommit", source: "github" });
  const hintFiles = [
    { path: "flake.lock", desc: "Current pinned revision for " + inputName },
    { path: "flake.nix", desc: "Input declaration" },
  ];
  const pill = html`
    <div style=${{ display: "inline-flex", alignItems: "center", gap: 6, padding: "4px 10px 4px 8px", background: T.color.surface2, border: "1px solid " + T.color.border, borderRadius: T.radius.sm, fontSize: T.fontSize.xs, fontFamily: T.font.mono, lineHeight: 1, transition: "all " + T.transition.fast }}>
      <span style=${{ color: T.color.pending400, fontWeight: 600, fontSize: 10, letterSpacing: "0.03em" }}>OVERRIDE</span>
      <span style=${{ width: 1, height: 12, background: T.color.border }} />
      <span style=${{ color: T.color.textSecondary, fontWeight: 500 }}>${inputName}</span>
      ${isGH && html`
        <span style=${{ width: 1, height: 12, background: T.color.border }} />
        <a href=${"https://github.com/" + owner + "/" + repo} target="_blank" rel="noopener" class="gh-link" style=${{ display: "inline-flex", alignItems: "center", gap: 4, fontSize: T.fontSize.xs }} onClick=${e => e.stopPropagation()}>
          <${GHIcon} size=${10} /> <span>${owner}/${repo}</span>
        </a>
      `}
      ${isGH && gitRef && html`
        <a href=${"https://github.com/" + owner + "/" + repo + "/commit/" + gitRef} target="_blank" rel="noopener" class="gh-link" style=${{ fontSize: T.fontSize.xs, color: T.color.saku300 }} onClick=${e => e.stopPropagation()}>${short}</a>
      `}
      ${pr && html`
        <a href=${"https://github.com/" + owner + "/" + repo + "/pull/" + pr} target="_blank" rel="noopener" class="gh-link" style=${{ display: "inline-flex", alignItems: "center", gap: 3, fontSize: T.fontSize.xs, color: T.color.running400, padding: "1px 5px", background: T.color.running400 + "12", borderRadius: T.radius.xs, border: "1px solid " + T.color.running400 + "20" }} onClick=${e => e.stopPropagation()}>PR #${pr}</a>
      `}
      ${!isGH && gitRef && html`<span style=${{ color: T.color.textTertiary }}>${gitRef}</span>`}
    </div>
  `;
  return html`<${DataHint} commands=${hintCmds} files=${hintFiles} position="below" notes="--override-input temporarily replaces the flake.lock pin for this input during evaluation.">${pill}</${DataHint}>`;
}

// ─── NIX BUILD ROW ──────────────────────────────────────────
function NixBuildRow({ build, isExpanded, onToggle }) {
  const cfg = statusConfig[build.status] || statusConfig.pending;
  const hasOv = build.overrideInputs && build.overrideInputs.length > 0;
  const commitUrl = build.owner && build.repo && build.commit
    ? "https://github.com/" + build.owner + "/" + build.repo + "/commit/" + build.commit : null;
  const shortSha = build.commit ? build.commit.slice(0, 7) : "—";
  const flakeTarget = build.flakeRef + "#" + build.derivation;
  const overrideArgs = (build.overrideInputs || []).map(oi =>
    " --override-input " + oi.inputName + " " + (oi.inputType === "github" ? "github:" + (oi.owner||"") + "/" + (oi.repo||"") + "/" + (oi.ref||oi.gitRef||"") : (oi.ref||oi.gitRef||""))
  ).join("");

  const statusHint = [
    { label: "Build & check exit code", cmd: "nix build " + flakeTarget + overrideArgs + "\necho $?", source: "nix" },
    { label: "Evaluate without building", cmd: "nix eval " + flakeTarget + " --raw 2>&1", source: "nix" },
  ];
  const derivationHint = [
    { label: "Show derivation path", cmd: "nix path-info --derivation " + flakeTarget, source: "nix" },
    { label: "List outputs", cmd: "nix derivation show " + flakeTarget + " | jq '.[].outputs'", source: "nix" },
  ];
  const branchHint = [
    { label: "Current branch", cmd: "git branch --show-current", source: "git" },
  ];
  const commitHint = [
    { label: "Commit details", cmd: "git log -1 --format='%H%n%s%n%an%n%ai' " + shortSha, source: "git" },
  ];
  const durationHint = [
    { label: "Build with timing", cmd: "time nix build " + flakeTarget + overrideArgs, source: "shell" },
  ];
  const logHintCmds = [
    { label: "Retrieve build log", cmd: "nix log " + flakeTarget, source: "nix" },
    { label: "Stream log during build", cmd: "nix build " + flakeTarget + overrideArgs + " -L 2>&1", source: "nix" },
  ];
  const nixCmdHintCmds = [
    { label: "Full build command", cmd: "nix build " + flakeTarget + overrideArgs, source: "nix" },
    { label: "Dry-run", cmd: "nix build " + flakeTarget + overrideArgs + " --dry-run 2>&1", source: "nix" },
  ];
  const flakeRefHint = [
    { label: "Flake metadata", cmd: "nix flake metadata " + build.flakeRef + " --json", source: "nix" },
  ];

  return html`
    <div style=${{ marginBottom: 2 }}>
      <div onClick=${onToggle} style=${{ display: "grid", gridTemplateColumns: "36px 1fr auto", alignItems: "center", gap: 12, padding: "12px 16px", background: isExpanded ? T.color.surface1 : "transparent", border: "1px solid " + (isExpanded ? T.color.border : "transparent"), borderBottom: isExpanded ? "none" : "1px solid " + T.color.borderSubtle, borderRadius: isExpanded ? T.radius.lg + " " + T.radius.lg + " 0 0" : "0", cursor: "pointer", transition: "all " + T.transition.base }}
        onMouseEnter=${e => { if (!isExpanded) e.currentTarget.style.background = T.color.surface1 + "80"; }}
        onMouseLeave=${e => { if (!isExpanded) e.currentTarget.style.background = "transparent"; }}>
        <${DataHint} commands=${statusHint} position="right" notes="Exit code 0 = passed, non-zero = failed.">
          <div style=${{ width: 32, height: 32, borderRadius: T.radius.md, background: cfg.bg, border: "1px solid " + cfg.color + "30", display: "flex", alignItems: "center", justifyContent: "center", fontSize: 14, color: cfg.color, fontWeight: 600, boxShadow: build.status === "running" ? T.shadow.glow(cfg.color) : "none" }}>
            <span style=${{ animation: build.status === "running" ? "spin 1.5s linear infinite" : "none", display: "inline-block" }}>${cfg.icon}</span>
          </div>
        </${DataHint}>
        <div style=${{ minWidth: 0 }}>
          <div style=${{ display: "flex", alignItems: "center", gap: 8, marginBottom: 3, flexWrap: "wrap" }}>
            <${DataHint} commands=${derivationHint} position="below" files=${[{ path: "flake.nix", desc: "Derivation defined via flake outputs" }]}>
              <span style=${{ fontFamily: T.font.mono, fontSize: T.fontSize.sm, fontWeight: 600, color: T.color.textPrimary }}>${build.derivation}</span>
            </${DataHint}>
            <${StatusBadge} status=${build.status} size="sm" />
            ${hasOv && html`<span style=${{ fontSize: 9, fontWeight: 600, fontFamily: T.font.mono, padding: "2px 6px", borderRadius: T.radius.xs, background: T.color.pending400 + "15", color: T.color.pending400, border: "1px solid " + T.color.pending400 + "20", letterSpacing: "0.04em" }}>⚑ ${build.overrideInputs.length} OVERRIDE${build.overrideInputs.length > 1 ? "S" : ""}</span>`}
            ${build.pr && html`<a href=${"https://github.com/" + build.owner + "/" + build.repo + "/pull/" + build.pr} target="_blank" rel="noopener" class="gh-link" style=${{ display: "inline-flex", alignItems: "center", gap: 3, fontFamily: T.font.mono, fontSize: 10, color: T.color.running400, padding: "2px 6px", background: T.color.running400 + "12", borderRadius: T.radius.xs, border: "1px solid " + T.color.running400 + "20" }} onClick=${e => e.stopPropagation()}>PR #${build.pr}</a>`}
          </div>
          <div style=${{ display: "flex", alignItems: "center", gap: 8, fontSize: T.fontSize.xs, color: T.color.textTertiary, flexWrap: "wrap" }}>
            <${DataHint} commands=${branchHint} position="below"><span style=${{ fontFamily: T.font.mono, padding: "1px 6px", background: T.color.surface3, borderRadius: T.radius.xs }}>⎇ ${build.branch || "main"}</span></${DataHint}>
            <${DataHint} commands=${commitHint} position="below">
              ${commitUrl ? html`<a href=${commitUrl} target="_blank" rel="noopener" class="gh-link" style=${{ fontFamily: T.font.mono, fontSize: T.fontSize.xs, display: "inline-flex", alignItems: "center", gap: 4 }} onClick=${e => e.stopPropagation()}><${GHIcon} size=${10} /> ${shortSha}</a>` : html`<span style=${{ fontFamily: T.font.mono }}>${shortSha}</span>`}
            </${DataHint}>
            <span>·</span>
            <${DataHint} commands=${flakeRefHint} position="below" files=${[{ path: "flake.nix", desc: "Flake source" }, { path: "flake.lock", desc: "Pinned deps" }]}><span>${build.flakeRef}</span></${DataHint}>
          </div>
        </div>
        <${DataHint} commands=${durationHint} position="above-right">
          <div style=${{ textAlign: "right", flexShrink: 0 }}>
            <div style=${{ fontFamily: T.font.mono, fontSize: T.fontSize.xs, color: T.color.textPrimary }}>${build.duration}</div>
            <div style=${{ fontSize: T.fontSize.xs, color: T.color.textTertiary, marginTop: 2 }}>${build.time}</div>
          </div>
        </${DataHint}>
      </div>
      ${isExpanded && html`
        <div style=${{ background: T.color.surface1, border: "1px solid " + T.color.border, borderTop: "none", borderRadius: "0 0 " + T.radius.lg + " " + T.radius.lg, padding: "0 16px 16px", animation: "fadeIn 200ms ease" }}>
          ${hasOv && html`
            <div style=${{ padding: "12px 0", borderBottom: "1px solid " + T.color.borderSubtle, marginBottom: 12 }}>
              <div style=${{ fontSize: 10, fontFamily: T.font.mono, color: T.color.textTertiary, textTransform: "uppercase", letterSpacing: "0.06em", marginBottom: 8 }}>Override Inputs</div>
              <div style=${{ display: "flex", flexWrap: "wrap", gap: 6 }}>
                ${build.overrideInputs.map((oi, i) => html`
                  <${OverridePill} key=${i} inputName=${oi.inputName} type=${oi.inputType || oi.type}
                    owner=${oi.owner} repo=${oi.repo} ref=${oi.gitRef || oi.ref} pr=${oi.pr}
                    flakeRef=${build.flakeRef} />
                `)}
              </div>
            </div>
          `}
          <${DataHint} commands=${nixCmdHintCmds} position="below" notes="The exact nix command invocation for this build.">
            <div style=${{ padding: "10px 12px", background: T.color.surface0, borderRadius: T.radius.sm, border: "1px solid " + T.color.borderSubtle, marginBottom: 12, overflowX: "auto", width: "100%" }}>
              <div style=${{ fontFamily: T.font.mono, fontSize: T.fontSize.xs, color: T.color.textSecondary, whiteSpace: "pre-wrap", wordBreak: "break-all" }}>
                <span style=${{ color: T.color.textTertiary }}>$ </span>
                <span style=${{ color: T.color.pass400 }}>nix build</span>
                <span> ${build.flakeRef}#${build.derivation}</span>
                ${(build.overrideInputs || []).map(oi => html`<span style=${{ color: T.color.pending400 }}>${" "}--override-input</span><span> ${oi.inputName} ${oi.inputType === "github" ? "github:" + (oi.owner||"") + "/" + (oi.repo||"") + "/" + (oi.gitRef||oi.ref||"") : (oi.gitRef||oi.ref||"")}</span>`)}
              </div>
            </div>
          </${DataHint}>
          <${DataHint} commands=${logHintCmds} files=${[{ path: "/nix/var/log/nix/drvs/", desc: "Cached build logs" }]} position="above" notes="Build logs streamed from nix daemon.">
            <div style=${{ background: T.color.surface0, borderRadius: T.radius.sm, border: "1px solid " + T.color.borderSubtle, overflow: "hidden", width: "100%" }}>
              <div style=${{ padding: "8px 12px", borderBottom: "1px solid " + T.color.borderSubtle, display: "flex", alignItems: "center", justifyContent: "space-between" }}>
                <span style=${{ fontFamily: T.font.mono, fontSize: T.fontSize.xs, color: T.color.textTertiary }}>build log · ${build.derivation}</span>
                <${StatusBadge} status=${build.status} size="sm" />
              </div>
              <div style=${{ padding: "6px 0", maxHeight: 280, overflowY: "auto" }}>
                ${build.log.map((line, i) => html`<${LogLine} key=${i} number=${line.n} content=${line.text} level=${line.level} />`)}
              </div>
            </div>
          </${DataHint}>
        </div>
      `}
    </div>
  `;
}

// ─── MAIN APP ───────────────────────────────────────────────
function App() {
  const [activeNav, setActiveNav] = useState("builds");
  const [expandedBuild, setExpandedBuild] = useState(null);
  const [filter, setFilter] = useState("all");
  const [search, setSearch] = useState("");
  const [builds, setBuilds] = useState(BUILDS_DATA);
  const [meta, setMeta] = useState(META_DATA);

  // Auto-refresh every 30s
  useEffect(() => {
    const interval = setInterval(async () => {
      try {
        const r = await fetch("/api/builds");
        if (r.ok) { const d = await r.json(); setBuilds(d.data); }
      } catch(_) {}
    }, 30000);
    return () => clearInterval(interval);
  }, []);

  const navItems = [
    { id: "builds", label: "Builds", icon: "⬡" },
    { id: "derivations", label: "Derivations", icon: "◇" },
    { id: "inputs", label: "Flake Inputs", icon: "⎇" },
    { id: "cache", label: "Cache", icon: "◈" },
  ];

  const filtered = useMemo(() => {
    let r = builds;
    if (filter !== "all") r = r.filter(b => b.status === filter);
    if (search) {
      const s = search.toLowerCase();
      r = r.filter(b =>
        b.derivation.toLowerCase().includes(s) ||
        (b.branch || "").toLowerCase().includes(s) ||
        b.flakeRef.toLowerCase().includes(s) ||
        b.commit.toLowerCase().includes(s) ||
        (b.overrideInputs || []).some(oi => oi.inputName.toLowerCase().includes(s))
      );
    }
    return r;
  }, [filter, search, builds]);

  const cnt = {
    all: builds.length,
    passed: builds.filter(b => b.status === "passed").length,
    failed: builds.filter(b => b.status === "failed").length,
    running: builds.filter(b => b.status === "running").length,
    pending: builds.filter(b => b.status === "pending").length,
  };
  const overrideCnt = builds.filter(b => b.overrideInputs && b.overrideInputs.length > 0).length;

  const successRateHint = [
    { label: "Aggregate from build results", cmd: "susui scan . --json | jq '.stats.success_rate'", source: "shell" },
  ];
  const runningHint = [
    { label: "List active nix builds", cmd: "ps aux | grep 'nix-build\\|nix build' | grep -v grep", source: "shell" },
  ];
  const overrideHint = [
    { label: "Grep for override flags", cmd: "grep -r 'override-input' .github/workflows/ ci/", source: "shell" },
  ];
  const failedHint = [
    { label: "Find all failed derivations", cmd: "susui scan . --json | jq '[.builds[] | select(.status==\"failed\") | .derivation]'", source: "shell" },
  ];

  return html`
    <div class="root" style=${{ display: "flex" }}>
      <style>${cssText}</style>
      <nav style=${{ width: 240, height: "100vh", position: "fixed", left: 0, top: 0, background: T.color.surface0, borderRight: "1px solid " + T.color.border, padding: "24px 16px", display: "flex", flexDirection: "column", zIndex: 50 }}>
        <div style=${{ marginBottom: 32, padding: "0 8px" }}>
          <div style=${{ display: "flex", alignItems: "center", gap: 10, marginBottom: 4 }}>
            <div style=${{ width: 32, height: 32, borderRadius: T.radius.sm, background: "linear-gradient(135deg, " + T.color.saku400 + ", " + T.color.saku600 + ")", display: "flex", alignItems: "center", justifyContent: "center", boxShadow: T.shadow.glow(T.color.saku400), overflow: "hidden" }}><${SakuIcon} size=${28} /></div>
            <div>
              <div style=${{ fontSize: T.fontSize.lg, fontWeight: 700, letterSpacing: "-0.02em", lineHeight: 1, fontFamily: T.font.display, color: T.color.textPrimary }}>nix builds</div>
              <div style=${{ fontSize: T.fontSize.xs, color: T.color.textTertiary, letterSpacing: "0.03em", marginTop: 2 }}>sus ui · 柵</div>
            </div>
          </div>
        </div>
        <div style=${{ margin: "0 8px 24px", padding: "6px 10px", background: T.color.surface2, borderRadius: T.radius.sm, fontSize: T.fontSize.xs, fontFamily: T.font.mono, color: T.color.textTertiary, display: "flex", alignItems: "center", gap: 6 }}>
          <span style=${{ width: 6, height: 6, borderRadius: "50%", background: cnt.running > 0 ? T.color.running400 : T.color.pass400, animation: cnt.running > 0 ? "pulse 1.5s infinite" : "none" }} />
          ${cnt.running > 0 ? cnt.running + " building" : "idle"} · ${cnt.all} total
        </div>
        <div style=${{ display: "flex", flexDirection: "column", gap: 2 }}>
          ${navItems.map(item => html`
            <a key=${item.id} onClick=${() => setActiveNav(item.id)}
              style=${{ padding: "8px 12px", borderRadius: T.radius.sm, fontSize: T.fontSize.md, color: activeNav === item.id ? T.color.textPrimary : T.color.textSecondary, background: activeNav === item.id ? T.color.surface2 : "transparent", textDecoration: "none", fontWeight: activeNav === item.id ? 500 : 400, transition: "all " + T.transition.fast, cursor: "pointer", display: "flex", alignItems: "center", gap: 8, borderLeft: activeNav === item.id ? "2px solid " + T.color.saku400 : "2px solid transparent" }}>
              <span style=${{ fontSize: 14, opacity: 0.7 }}>${item.icon}</span>
              ${item.label}
            </a>
          `)}
        </div>
        <div style=${{ flex: 1 }} />
        <div style=${{ padding: "10px 8px", marginBottom: 8, background: T.color.hint + "08", borderRadius: T.radius.sm, border: "1px solid " + T.color.hint + "15" }}>
          <div style=${{ display: "flex", alignItems: "center", gap: 6, marginBottom: 6 }}>
            <div style=${{ width: 5, height: 5, borderRadius: "50%", background: T.color.hint, opacity: 0.7 }} />
            <span style=${{ fontSize: 10, fontFamily: T.font.mono, color: T.color.hint, fontWeight: 500, letterSpacing: "0.04em" }}>DATA SOURCE HINTS</span>
          </div>
          <div style=${{ fontSize: 10, color: T.color.textTertiary, lineHeight: 1.5 }}>Hover any element to see the nix, git, and shell commands used to source its data.</div>
        </div>
        <div style=${{ padding: "12px 8px", borderTop: "1px solid " + T.color.border, fontSize: T.fontSize.xs, color: T.color.textTertiary, lineHeight: 1.6 }}>Built with precision.<br />柵 — cut clean, ship fast.</div>
      </nav>
      <main style=${{ marginLeft: 240, flex: 1, padding: "48px 56px", maxWidth: 960 }}>
        <div class="animate-in" style=${{ marginBottom: 32 }}>
          <div style=${{ display: "flex", alignItems: "center", gap: 10, marginBottom: 6 }}>
            <div style=${{ width: 3, height: 18, borderRadius: 2, background: T.color.saku400 }} />
            <h1 style=${{ fontSize: T.fontSize["2xl"], fontWeight: 600, letterSpacing: "-0.02em", color: T.color.textPrimary, fontFamily: T.font.display }}>Nix Builds</h1>
          </div>
          <p style=${{ fontSize: T.fontSize.base, color: T.color.textSecondary, marginLeft: 13, maxWidth: 540 }}>
            All flake builds, derivations, override inputs, and evaluation logs. Hover any element for data source commands.
          </p>
        </div>
        <div class="animate-in stagger-1" style=${{ display: "flex", gap: 12, marginBottom: 32, flexWrap: "wrap" }}>
          <${MetricCard} label="Success Rate" value=${cnt.all > 0 ? Math.round(cnt.passed / cnt.all * 100) : 0} suffix="%"
            color=${T.color.pass400} hintCommands=${successRateHint} hintNotes="Calculated as passed / total builds." />
          <${MetricCard} label="Running" value=${cnt.running}
            sub=${cnt.running > 0 ? "in progress" : "idle"}
            color=${cnt.running > 0 ? T.color.running400 : null}
            hintCommands=${runningHint} />
          <${MetricCard} label="Overridden" value=${overrideCnt} sub="builds with --override-input"
            hintCommands=${overrideHint} />
          <${MetricCard} label="Failed" value=${cnt.failed}
            sub=${cnt.failed > 0 ? "needs attention" : "all clear"}
            color=${cnt.failed > 0 ? T.color.fail400 : null}
            hintCommands=${failedHint} />
        </div>
        <div class="animate-in stagger-2" style=${{ display: "flex", alignItems: "center", justifyContent: "space-between", marginBottom: 16, gap: 16, flexWrap: "wrap" }}>
          <div style=${{ display: "flex", gap: 4, flexWrap: "wrap" }}>
            <${FilterTab} label="All" count=${cnt.all} active=${filter === "all"} onClick=${() => setFilter("all")} />
            <${FilterTab} label="Passed" count=${cnt.passed} active=${filter === "passed"} onClick=${() => setFilter("passed")} color=${T.color.pass400} />
            <${FilterTab} label="Failed" count=${cnt.failed} active=${filter === "failed"} onClick=${() => setFilter("failed")} color=${T.color.fail400} />
            <${FilterTab} label="Running" count=${cnt.running} active=${filter === "running"} onClick=${() => setFilter("running")} color=${T.color.running400} />
            <${FilterTab} label="Pending" count=${cnt.pending} active=${filter === "pending"} onClick=${() => setFilter("pending")} color=${T.color.pending400} />
          </div>
          <div style=${{ width: 260 }}>
            <${SearchInput} placeholder="Search builds, inputs..." value=${search} onChange=${e => setSearch(e.target.value)} />
          </div>
        </div>
        <div class="animate-in stagger-3" style=${{ background: T.color.surface0, border: "1px solid " + T.color.border, borderRadius: T.radius.xl, padding: "8px", marginBottom: 40 }}>
          ${filtered.length === 0 && html`
            <div style=${{ padding: 40, textAlign: "center", color: T.color.textTertiary, fontSize: T.fontSize.sm }}>No builds match the current filter.</div>
          `}
          ${filtered.map(build => html`
            <${NixBuildRow} key=${build.id} build=${build} isExpanded=${expandedBuild === build.id} onToggle=${() => setExpandedBuild(expandedBuild === build.id ? null : build.id)} />
          `)}
        </div>
        <div style=${{ marginTop: 40, paddingTop: 24, borderTop: "1px solid " + T.color.border, display: "flex", justifyContent: "space-between", alignItems: "center" }}>
          <div style=${{ fontSize: T.fontSize.xs, color: T.color.textTertiary }}>sus ui · nix build dashboard</div>
          <div style=${{ fontSize: T.fontSize.xs, color: T.color.textTertiary, fontFamily: T.font.mono }}>柵 /saku/</div>
        </div>
      </main>
    </div>
  `;
}

render(html`<${App} />`, document.body);
</script>
</body>
</html>"##;
