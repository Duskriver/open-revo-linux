import React from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';
import ReactDOM from 'react-dom/client';
import App from './App';
import LinuxDashboard from './components/LinuxDashboard';
import OsdOverlay from './components/OsdOverlay';
import { WidgetHost } from './components/WidgetHost';
import './index.css';

const params = new URLSearchParams(window.location.search);
const injectedWidgetId = (window as any).__OPENREVO_WIDGET_ID__ as string | undefined;
const isOsd = !!(window as any).__OPENREVO_OSD_WINDOW__ || params.get('window') === 'osd' || window.location.hash === '#/osd';
const isWidget = !!injectedWidgetId || params.get('window') === 'widget' || window.location.hash.startsWith('#/widget');

const widgetId = injectedWidgetId || params.get('id') || (window.location.hash.split('/')[2] ?? 'core');

async function mount() {
  const platform = isTauri()
    ? await invoke<string>('get_platform')
    : params.get('platform') ?? (/Linux/.test(navigator.userAgent) ? 'linux' : 'windows');
  const isLinux = platform === 'linux';
  ReactDOM.createRoot(document.getElementById('root') as HTMLElement).render(
    <React.StrictMode>
      {isLinux ? (
        <LinuxDashboard />
      ) : isOsd ? (
        <OsdOverlay />
      ) : isWidget ? (
        <WidgetHost pluginId={widgetId} />
      ) : (
        <App />
      )}
    </React.StrictMode>,
  );
}
void mount().catch(error => {
  const root = document.getElementById('root');
  if (root) root.textContent = `OpenRevo 启动失败：${String(error)}`;
});
