import { ipcRenderer } from 'electron';
import { t } from '../../../core/i18n';

// embyWall/modals/telemetry.ts — [v1.12.0] 匿名使用统计卡（设置面板「关于」页）。
//
// 桌面版（AniBox 的 modals/telemetry.ts）同款卡片的网页端移植：开关 / 上次上报结果 /
// 重置匿名 ID / 手动上报一次。全部动作由用户显式触发，没有任何自动上传路径 ——
// 自动心跳只在「今天确实有人打开过增强页面」且开关开着时发生一次。
//
// 与桌面版的差异（都是网页端架构决定的）：
//  - 上报由 NAS 后端（Go）发出，不走浏览器：所以面板状态必须读后端 /api/stats，
//    浏览器拿不到也无需拿到完整匿名 ID（后端只回前 8 位）。
//  - 触发点从「应用启动」改成「反代注入成功」：装了却没人用的 NAS 不算活跃。

const MUTED = 'var(--fnos-ui-muted,#999)';

/** 小开关（与「外观」页同款：隐藏 input + 轨道 + 滑块） */
function mkToggle(on: boolean, onChange: (v: boolean) => void): { el: HTMLElement; set: (v: boolean) => void } {
  const label = document.createElement('label');
  label.style.cssText = 'position:relative;display:inline-block;width:42px;height:23px;cursor:pointer;flex:none;';
  const input = document.createElement('input');
  input.type = 'checkbox';
  input.checked = on;
  input.style.cssText = 'position:absolute;opacity:0;width:0;height:0;';
  const track = document.createElement('span');
  track.style.cssText = 'position:absolute;inset:0;border-radius:23px;background:rgba(140,140,160,.45);transition:.2s;';
  const knob = document.createElement('span');
  knob.style.cssText = 'position:absolute;top:2.5px;left:2.5px;width:18px;height:18px;border-radius:50%;'
    + 'background:#fff;transition:.2s;box-shadow:0 1px 3px rgba(0,0,0,.3);';
  label.appendChild(input);
  label.appendChild(track);
  label.appendChild(knob);
  const paint = (): void => {
    track.style.background = input.checked ? 'var(--fnos-ui-accent)' : 'rgba(140,140,160,.45)';
    knob.style.left = input.checked ? '21.5px' : '2.5px';
  };
  paint();
  input.addEventListener('change', () => { paint(); onChange(input.checked); });
  return { el: label, set: (v: boolean) => { input.checked = v; paint(); } };
}

function mkRow(title: string, right?: HTMLElement): HTMLElement {
  const row = document.createElement('div');
  row.style.cssText = 'display:flex;justify-content:space-between;align-items:center;gap:10px;margin-top:10px;';
  const span = document.createElement('span');
  span.style.cssText = 'font-weight:600;letter-spacing:.5px;';
  span.textContent = t(title);
  row.appendChild(span);
  if (right) row.appendChild(right);
  return row;
}

function mkNote(text: string): HTMLElement {
  const d = document.createElement('div');
  d.style.cssText = 'font-size:11px;color:' + MUTED + ';line-height:1.6;margin-top:6px;';
  d.textContent = t(text);
  return d;
}

/**
 * [v1.12.0] 匿名使用统计卡（「关于」页）。
 * 只显示/控制：开关、上次上报结果、重置匿名 ID、手动上报一次。
 */
export function buildStatsCard(): HTMLElement {
  const card = document.createElement('div');
  card.style.cssText = 'width:100%;max-width:440px;text-align:left;margin-top:14px;padding:12px 14px;border-radius:12px;'
    + 'background:var(--fnos-ui-input-bg)!important;border:1px solid var(--fnos-ui-border3);';

  const wrap = document.createElement('div');

  const toggle = mkToggle(true, (v) => {
    ipcRenderer.invoke('stats:set-enabled', v).catch(() => {});
  });

  // ── 可折叠头部：默认收起，点击标题栏展开/收起（保留原「📊 匿名使用统计」标题）──
  const header = document.createElement('button');
  header.type = 'button';
  header.setAttribute('aria-expanded', 'false');
  header.style.cssText = 'display:flex;align-items:center;justify-content:space-between;gap:8px;'
    + 'width:100%;padding:0;margin:0;background:none;border:0;cursor:pointer;color:inherit;font:inherit;text-align:left;';

  const title = document.createElement('div');
  title.style.cssText = 'font-size:12.5px;font-weight:700;color:var(--fnos-ui-pill-text);';
  title.textContent = t('📊 匿名使用统计');
  header.appendChild(title);

  const chevron = document.createElement('span');
  chevron.setAttribute('aria-hidden', 'true');
  chevron.textContent = '▸';
  chevron.style.cssText = 'flex:none;font-size:12px;line-height:1;color:' + MUTED + ';'
    + 'transition:transform .2s ease;transform:rotate(0deg);';
  header.appendChild(chevron);

  const body = document.createElement('div');
  body.id = 'fnos-stats-body';
  header.setAttribute('aria-controls', body.id);
  body.style.cssText = 'overflow:hidden;max-height:0;opacity:0;'
    + 'transition:max-height .25s ease,opacity .2s ease;';
  body.appendChild(mkRow('参与匿名统计', toggle.el));
  body.appendChild(mkNote('开启后收集必须的应用版本 + 系统类型，用于日志反馈收集需要的系统信息，方便排查故障 Bug。'
    + '不涉及账号、IP、媒体库及文件路径等隐私数据，服务端亦不做 IP 存储，可随时在这里关闭。'));

  let open = false;
  const applyOpen = (): void => {
    header.setAttribute('aria-expanded', open ? 'true' : 'false');
    chevron.style.transform = open ? 'rotate(90deg)' : 'rotate(0deg)';
    if (open) {
      body.style.maxHeight = body.scrollHeight + 'px';
      body.style.opacity = '1';
    } else {
      // 从 max-height:none 收起前先固定为像素值，否则无法过渡
      if (body.style.maxHeight === 'none') {
        body.style.maxHeight = body.scrollHeight + 'px';
        void body.offsetHeight;
      }
      body.style.maxHeight = '0px';
      body.style.opacity = '0';
    }
  };
  header.addEventListener('click', () => { open = !open; applyOpen(); });
  body.addEventListener('transitionend', (e: TransitionEvent) => {
    if (e.propertyName === 'max-height' && open) body.style.maxHeight = 'none';
  });

  wrap.appendChild(header);
  wrap.appendChild(body);

  // [lc-1250] 精简：移除「立即上报一次」「重置匿名 ID」按钮（匿名 ID 已是机器级固定, 重置无意义）
  // [lc-1250-web] 状态行整行不显示（读取中/上报结果/计划提示等一律不展示），仅保留开关同步
  ipcRenderer.invoke('stats:get-info').then((s: any) => {
    if (!s) return;
    toggle.set(s.enabled !== false);
  }).catch(() => {});

  card.appendChild(wrap);
  return card;
}

/**
 * [v1.13.0] 「Bug 反馈 + 日志上传」卡已移除：反馈入口统一为侧栏「加入Q群」。
 */
