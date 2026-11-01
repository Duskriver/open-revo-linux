import { useCallback, useEffect, useRef, useState } from 'react';
import { linuxApi } from '../lib/linuxApi';
import type { LinuxAction, LinuxMutation, LinuxMutationArgs } from '../lib/linuxApi';
import type { LinuxSnapshot } from '../types/linux';

/** Own polling and mutations outside the presentation tree.
 * A status read also renews the backend's control lease, even on non-control tabs.
 */
export function useLinuxBackend() {
  const [status, setStatus] = useState<LinuxSnapshot | null>(null);
  const [connectionError, setConnectionError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [autostart, setAutostart] = useState<boolean | null>(null);
  const pending = useRef<Promise<void> | null>(null);
  const acting = useRef(false);
  const mounted = useRef(true);
  const native = linuxApi.isNative();

  const refresh = useCallback((): Promise<void> => {
    if (pending.current) return pending.current;
    if (!native) {
      setConnectionError('请通过 pnpm linux:dev 启动桌面程序，浏览器预览无法读取本机硬件。');
      return Promise.resolve();
    }
    const task = (async () => {
      try {
        const next = await linuxApi.getStatus();
        if (mounted.current) { setStatus(next); setConnectionError(null); }
      } catch (e) {
        if (mounted.current) setConnectionError(String(e));
      } finally {
        pending.current = null;
      }
    })();
    pending.current = task;
    return task;
  }, [native]);

  useEffect(() => {
    mounted.current = true;
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      await refresh();
      if (!cancelled) timer = setTimeout(poll, 2000);
    };
    void poll();
    if (native) linuxApi.getAutostart().then(v => {
      if (mounted.current) setAutostart(v);
    }).catch(e => {
      if (mounted.current) setError(`读取登录启动设置失败：${e}`);
    });
    return () => { cancelled = true; mounted.current = false; clearTimeout(timer); };
  }, [native, refresh]);

  const act: LinuxAction = useCallback(async <C extends LinuxMutation>(
    command: C, args: LinuxMutationArgs[C],
  ): Promise<boolean> => {
    if (acting.current || !native) return false;
    acting.current = true;
    setBusy(true);
    setError(null);
    let success = false;
    try { await linuxApi.mutate(command, args); success = true; }
    catch (e) { if (mounted.current) setError(String(e)); }
    finally {
      // A poll started before a write cannot confirm it. Wait for that poll,
      // then request fresh state, including after a partially failed command.
      if (pending.current) await pending.current;
      await refresh();
      acting.current = false;
      if (mounted.current) setBusy(false);
    }
    return success;
  }, [native, refresh]);

  const setAutostartEnabled = useCallback(async (enabled: boolean) => {
    if (await act('set_autostart_enabled', { enabled })) {
      if (mounted.current) setAutostart(enabled);
    }
  }, [act]);

  const clearError = useCallback(() => setError(null), []);
  return { status, connectionError, error, busy, autostart, native, refresh, act, clearError, setAutostartEnabled };
}
