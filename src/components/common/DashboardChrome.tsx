import { ReactNode, useEffect, useRef, useState } from 'react';
import { Palette } from 'lucide-react';
import logo from '../../assets/logo.png';

export interface DashboardTab {
  id: string;
  name: string;
  icon: ReactNode;
  className?: string;
  badge?: ReactNode;
  description?: string;
}

/** Shared Windows/Linux titlebar and navigation; platform side effects stay in callers. */
export function DashboardChrome({ theme, onToggleTheme, versionLabel, tabs, activeTab, onSelectTab,
  actions, mock = false, onToggleMaximize }: {
  theme: 'mono' | 'cyber'; onToggleTheme?: () => void; versionLabel?: string;
  tabs: DashboardTab[]; activeTab: string; onSelectTab: (id: string) => void;
  actions?: ReactNode; mock?: boolean; onToggleMaximize?: () => void;
}) {
  const nav = useRef<HTMLDivElement>(null);
  const [left, setLeft] = useState(false);
  const [right, setRight] = useState(false);
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout>>();
  const checkScroll = () => {
    const el = nav.current;
    if (el) { setLeft(el.scrollLeft > 2); setRight(el.scrollLeft + el.clientWidth < el.scrollWidth - 2); }
  };
  useEffect(() => {
    const el = nav.current;
    if (!el) return;
    const observer = new ResizeObserver(checkScroll);
    observer.observe(el);
    return () => { observer.disconnect(); clearTimeout(timer.current); };
  }, []);
  useEffect(() => {
    nav.current?.querySelector('.nav-tab.active')?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
    checkScroll();
  }, [activeTab, tabs.length]);
  return <>
    <div className="mech-titlebar shell-titlebar" data-slot="titlebar" data-tauri-drag-region onDoubleClick={onToggleMaximize}>
      <div className="logo-badge shared-brand" data-tauri-drag-region>
        <img src={logo} alt="OpenRevo Logo" data-tauri-drag-region />
        <span className="shared-brand-name" data-tauri-drag-region>OPENREVO</span>
        <button className="badge-tag" title={`复制版本号: v${__APP_VERSION__}`} onClick={async () => {
          try {
            await navigator.clipboard.writeText(`v${__APP_VERSION__}`);
            setCopied(true); clearTimeout(timer.current); timer.current = setTimeout(() => setCopied(false), 1500);
          } catch { setCopied(false); }
        }}>{copied ? '已复制' : `${versionLabel ?? 'PRO'} v${__APP_VERSION__}`}</button>
        {mock && <span className="shared-mock-badge">SAFE MOCK</span>}
      </div>
      <div className="shared-titlebar-drag" data-tauri-drag-region />
      <div className="shared-titlebar-actions" onMouseDown={e => e.stopPropagation()} onDoubleClick={e => e.stopPropagation()}>
        {onToggleTheme && <button className={`toggle-btn ${theme === 'cyber' ? 'active-cyan' : ''}`}
          aria-label="切换主题" title={theme === 'mono' ? '切换到多彩主题' : '切换到单色主题'} onClick={onToggleTheme}><Palette size={14} /></button>}
        {actions}
      </div>
    </div>
    <div className="sub-nav-wrapper shell-navbar" data-slot="navbar">
      {left && <div className="sub-nav-fade sub-nav-fade-left" />}
      <div ref={nav} className="sub-nav-bar" role="tablist" aria-label="功能页面" onScroll={checkScroll}
        onWheel={e => { if (e.deltaY) e.currentTarget.scrollLeft += e.deltaY; }}>
        {tabs.map(tab => <button key={tab.id} role="tab" aria-selected={activeTab === tab.id}
          className={`nav-tab ${tab.className ?? ''} ${activeTab === tab.id ? 'active' : ''}`}
          title={tab.description} onClick={() => onSelectTab(tab.id)}>{tab.icon}<span>{tab.name}</span>{tab.badge}</button>)}
      </div>
      {right && <div className="sub-nav-fade sub-nav-fade-right" />}
    </div>
  </>;
}
