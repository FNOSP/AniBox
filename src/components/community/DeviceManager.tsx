/* [lc-2402] 设备管理页 —— 信息结构按用户给的截图逐项还原：
 *   标题「设备管理」+ 副行「免费 · 共 N 台设备处于登录状态」
 *   行 = 系统图标 + 「Windows · 3840×2160 · 150%」+ [本机] 角标
 *        第二行 IP 网段 + 标识（设备码前 8 位大写）+ 最后活跃
 * 非本机行有「登出此设备」；顶部有「登出其他设备」。 */
import { useCallback, useEffect, useState } from "react";
import {
  LogOut as IcoLogout,
  Monitor as IcoMonitor,
  Smartphone as IcoPhone,
} from "lucide-react";
import {
  deviceTag,
  fetchDevices,
  revokeDevice,
  revokeOtherDevices,
  type CommunityDevice,
} from "../../lib/community";

function fmtRel(ms: number): string {
  if (!ms) return "—";
  const diff = Date.now() - ms;
  if (diff < 60_000) return "刚刚";
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)} 分钟前`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)} 小时前`;
  if (diff < 30 * 86_400_000) return `${Math.floor(diff / 86_400_000)} 天前`;
  const d = new Date(ms);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}

export function DeviceManager() {
  const [devices, setDevices] = useState<CommunityDevice[] | null>(null);
  const [err, setErr] = useState("");
  const [busyId, setBusyId] = useState("");

  const reload = useCallback(async () => {
    try {
      setDevices(await fetchDevices());
    } catch (e) {
      setErr(String(e));
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  const revoke = async (id: string) => {
    setBusyId(id);
    setErr("");
    try {
      await revokeDevice(id);
      await reload();
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusyId("");
    }
  };

  const revokeOthers = async () => {
    setBusyId("__others__");
    setErr("");
    try {
      await revokeOtherDevices();
      await reload();
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusyId("");
    }
  };

  const count = devices?.length ?? 0;
  const hasOther = (devices ?? []).some((d) => !d.current);

  return (
    <section className="cv2-card">
      <div className="cv2-dev-head">
        <h3 className="cv2-card-title">设备管理</h3>
        <span className="cv2-dev-sub">
          免费 · 共 {count} 台设备处于登录状态
        </span>
        {hasOther && (
          <button
            type="button"
            className="cmlink cv2-dev-others"
            onClick={() => void revokeOthers()}
            disabled={busyId === "__others__"}
          >
            {busyId === "__others__" ? "登出中…" : "登出其他设备"}
          </button>
        )}
      </div>

      {devices === null && !err && <div className="cv2-dev-empty">加载中…</div>}
      {err && <div className="set-warn cv2-warn">{err}</div>}

      <div className="cv2-dev-list">
        {(devices ?? []).map((d) => (
          <div className="cv2-dev-row" key={d.id}>
            <span className="cv2-dev-ico" aria-hidden>
              {d.os === "Windows" ? <IcoMonitor size={16} strokeWidth={1.8} /> : <IcoPhone size={16} strokeWidth={1.8} />}
            </span>
            <div className="cv2-dev-main">
              <div className="cv2-dev-line1">
                <span className="cv2-dev-os">{d.os}</span>
                <span className="cv2-dev-sep">·</span>
                <span>{d.screen}</span>
                <span className="cv2-dev-sep">·</span>
                <span>{d.scale}%</span>
                {d.current && <span className="cv2-dev-self">本机</span>}
              </div>
              <div className="cv2-dev-line2">
                <span>IP {d.ipNet}</span>
                <span className="cv2-dev-sep">标识 {deviceTag(d.deviceCode)}</span>
                <span>最后活跃 {fmtRel(d.lastSeenAt)}</span>
              </div>
            </div>
            {!d.current && (
              <button
                type="button"
                className="cmlink cv2-dev-out"
                onClick={() => void revoke(d.id)}
                disabled={busyId === d.id}
                title="登出此设备"
              >
                <IcoLogout size={13} strokeWidth={2} />
                {busyId === d.id ? "登出中…" : "登出"}
              </button>
            )}
          </div>
        ))}
      </div>
    </section>
  );
}
