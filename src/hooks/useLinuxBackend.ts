import { useCallback, useEffect, useRef, useState } from 'react';
import { errorMessage, linuxApi } from '../lib/linuxApi';
import type { LinuxAction, LinuxMutation, LinuxMutationArgs } from '../lib/linuxApi';
import type { LinuxSnapshot } from '../types/linux';

const STALE_AFTER_MS = 10_000;

/** Telemetry is read-only; the main window renews its controls separately. */
export function useLinuxBackend() {
  const [status, setStatus] = useState<LinuxSnapshot | null>(null);
  const [connectionError, setConnectionError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [leaseError, setLeaseError] = useState<string | null>(null);
  const [ageSeconds, setAgeSeconds] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);
  const [autostart, setAutostart] = useState<boolean | null>(null);
  const pending = useRef<Promise<boolean> | null>(null);
  const acting = useRef(false);
  const mounted = useRef(true);
  const receivedAt = useRef<number | null>(null);
  const native = linuxApi.isNative();

  const refresh = useCallback((): Promise<boolean> => {
    if (pending.current) return pending.current;
    if (!native) {
      setConnectionError('请通过 pnpm linux:dev 启动桌面程序，浏览器预览无法读取本机硬件。');
      return Promise.resolve(false);
    }
    const task = (async () => {
      try {
        const next = await linuxApi.getStatus();
        if (mounted.current) {
          receivedAt.current = performance.now();
          setAgeSeconds(0);
          setStatus(next);
          setConnectionError(null);
        }
        return true;
      } catch (e) {
        if (mounted.current) setConnectionError(errorMessage(e));
        return false;
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
    let leaseTimer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      await refresh();
      if (!cancelled) timer = setTimeout(poll, 2000);
    };
    void poll();
    const renewLease = async () => {
      try {
        if (receivedAt.current === null) return;
        if (performance.now() - receivedAt.current >= STALE_AFTER_MS) {
          if (!cancelled) setLeaseError('状态数据已过期，已停止续租；控制将按超时策略恢复');
          return;
        }
        await linuxApi.renewControlLease();
        if (!cancelled) setLeaseError(null);
      } catch (e) {
        if (!cancelled) setLeaseError(errorMessage(e));
      } finally {
        if (!cancelled) leaseTimer = setTimeout(renewLease, 2000);
      }
    };
    if (native) void renewLease();
    const ageTimer = setInterval(() => {
      if (receivedAt.current !== null) setAgeSeconds(Math.floor((performance.now() - receivedAt.current) / 1000));
    }, 1000);
    if (native) linuxApi.getAutostart().then(v => {
      if (mounted.current) setAutostart(v);
    }).catch(e => {
      if (mounted.current) setError(`读取登录启动设置失败：${errorMessage(e)}`);
    });
    return () => {
      cancelled = true; mounted.current = false;
      clearTimeout(timer); clearTimeout(leaseTimer); clearInterval(ageTimer);
    };
  }, [native, refresh]);

  const act: LinuxAction = useCallback(async <C extends LinuxMutation>(
    command: C, args: LinuxMutationArgs[C],
  ): Promise<boolean> => {
    if (acting.current || !native) return false;
    const releasing = command === 'restore_linux_oem_fan_curve'
      || ((command === 'set_linux_fan_boost' || command === 'set_linux_oem_fan_boost') && 'enabled' in args && args.enabled === false);
    if (!releasing && command !== 'set_autostart_enabled' && (connectionError || leaseError || receivedAt.current === null
      || performance.now() - receivedAt.current >= STALE_AFTER_MS)) {
      setError('硬件状态或控制连接尚未就绪，请等待刷新成功后再操作');
      return false;
    }
    acting.current = true;
    setBusy(true);
    setError(null);
    let success = false;
    try { await linuxApi.mutate(command, args); success = true; }
    catch (e) { if (mounted.current) setError(errorMessage(e)); }
    finally {
      // A poll started before a write cannot confirm it. Wait for that poll,
      // then request fresh state, including after a partially failed command.
      const confirmation = (async () => {
        if (pending.current) await pending.current;
        return refresh();
      })();
      let timeout: ReturnType<typeof setTimeout> | undefined;
      const confirmed = await Promise.race([
        confirmation,
        new Promise<false>(resolve => { timeout = setTimeout(() => resolve(false), 5000); }),
      ]);
      clearTimeout(timeout);
      if (!confirmed && success) {
        success = false;
        if (mounted.current) setError('操作请求已完成，但未能确认当前状态；请核对状态后再调节');
      }
      acting.current = false;
      if (mounted.current) setBusy(false);
    }
    return success;
  }, [native, refresh, connectionError, leaseError]);

  const setAutostartEnabled = useCallback(async (enabled: boolean) => {
    if (await act('set_autostart_enabled', { enabled })) {
      if (mounted.current) setAutostart(enabled);
    }
  }, [act]);

  const clearError = useCallback(() => setError(null), []);
  const stale = status !== null && (ageSeconds ?? 0) * 1000 >= STALE_AFTER_MS;
  return { status, connectionError, leaseError, stale, ageSeconds, error, busy, autostart, native, refresh, act, clearError, setAutostartEnabled };
}
