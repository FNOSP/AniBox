/* [lc-2115] 18 禁站源年龄确认弹窗（短剧页）。
   红果之后的站源（剧果/禁果短剧/野果/黑桃/黄豆）含成人内容，**首次切到这些站源**时弹一次
   完整条款确认；同意后写入 localStorage，之后不再打扰（同一浏览器永久有效）。

   设计口径：
   · 复用 lc-1607 免责声明那套弹窗外壳（`.ddet-mask` 遮罩 + `.ddet` 卡片 + `.dsc-*` 按钮），
     与全站弹层视觉一致，不引入新配色；
   · 与首次启动的免责声明**分开存键**（`anibox-adult-ack-v1`）：那是通用条款，这是成人内容
     专项声明，各管各的，互不覆盖；
   · 是「同意闸门」语义（同 DisclaimerModal 的 first 态）：不响应 Esc、点遮罩不关闭、
     只有「返回」和「我已年满十八岁」两个显式出口 —— 避免误触穿越；
   · 关闭动画照 lc-1603 口径：先挂 `dsc-mask-closing`（170ms）再真卸载。 */
import { useState } from "react";
import { createPortal } from "react-dom";

/** 已确认成人的 localStorage 键（置位后不再弹） */
export const ADULT_ACK_KEY = "anibox-adult-ack-v1";

/** 需要年龄确认的站源（红果之后的全部站源；红果自身为公开短剧内容，不拦） */
export const ADULT_SOURCES = new Set(["jinguo", "jinguoduanju", "yeguo", "heitao", "huangdou"]);

export function adultAckDone(): boolean {
  try {
    return localStorage.getItem(ADULT_ACK_KEY) === "1";
  } catch {
    // localStorage 不可用（隐私模式）：按已确认处理，不反复弹窗（无法持久化就没有意义）
    return true;
  }
}

export function markAdultAck() {
  try {
    localStorage.setItem(ADULT_ACK_KEY, "1");
  } catch {
    /* 存储被禁：本次会话内不再弹（调用方另有 state 兜底） */
  }
}

const ADULT_TITLE = "成人内容提示";

const ADULT_TEXT = `即将进入的站源包含成人内容，仅限年满十八周岁的成年人浏览。请您在继续前仔细阅读以下条款：

一、内容性质
1、该站源收录的短剧可能包含性暗示、性行为描写或其他成人题材内容，属于成人向作品。
2、本软件不制作、不审核、不存储、不分发上述内容，也不对其合法性、真实性与适当性作出任何保证。所有内容均来自您自行选择的第三方站点，与本软件及作者无关。

二、年龄与身份声明
1、您必须是年满十八周岁的完全民事行为能力人，方可继续访问该站源。
2、您不得以任何方式向未成年人展示、传播、分享该站源的内容，也不得协助未成年人绕过本提示。
3、如您所在国家或地区的法律法规对成人内容的浏览、传播设有年龄限制或禁止性规定，您必须遵守当地法律，并在不违反当地法律的前提下使用本功能。

三、责任承担
1、您完全自愿选择访问该站源，并对自己的一切使用行为及其后果承担全部责任。
2、因您访问、观看、传播上述内容而产生的任何法律责任、纠纷、损失或损害，均由您本人承担，本软件作者与贡献者不承担任何责任。
3、您同意不因上述内容向本软件作者或贡献者提出任何形式的索赔。

四、其他
1、本提示内容可能随时更新，不另行通知。
2、点击下方「本人已满十八岁」即表示您已完整阅读、充分理解并同意上述全部条款；如不同意，请点击「返回」。

如您未满十八周岁，或不同意上述任一条款，请立即离开。`;

export function AdultGateModal({
  onAgree,
  onBack,
}: {
  /** 确认年满十八岁：调用方负责写入 localStorage 并继续切换站源 */
  onAgree: () => void;
  /** 返回：不切换站源 */
  onBack: () => void;
}) {
  const [closing, setClosing] = useState(false);

  // 关闭动画：先挂 closing 类，170ms 后执行真正的动作（对齐 dsc 系列 CSS）
  const run = (fn: () => void) => {
    if (closing) return;
    setClosing(true);
    window.setTimeout(fn, 170);
  };

  return createPortal(
    <div className={`ddet-mask${closing ? " dsc-mask-closing" : ""}`}>
      <div className="ddet dsc-modal" onClick={(e) => e.stopPropagation()}>
        <header className="ddet-hd">
          <div className="ddet-hdm">
            <h2>{ADULT_TITLE}</h2>
          </div>
        </header>
        <div className="ddet-body dsc-body">
          <p className="dsc-text">{ADULT_TEXT}</p>
        </div>
        <div className="dsc-acts">
          <button className="dsc-btn ghost" onClick={() => run(onBack)}>
            返回
          </button>
          <button className="dsc-btn primary" onClick={() => run(onAgree)}>
            本人已满十八岁
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
