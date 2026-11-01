import React from 'react';
import ReactDOM from 'react-dom/client';
import LinuxDashboard from './components/LinuxDashboard';
import './index.css';

try {
  const root = document.getElementById('root');
  if (!root) throw new Error('缺少应用挂载节点');
  ReactDOM.createRoot(root).render(
    <React.StrictMode>
      <LinuxDashboard />
    </React.StrictMode>,
  );
} catch (error) {
  const root = document.getElementById('root');
  if (root) root.textContent = `OpenRevo 启动失败：${String(error)}`;
}
