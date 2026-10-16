/* [lc-1607] 免责声明弹窗（首次启动同意闸门 + 设置内只读查看）。
   外壳复用仓库弹层（.ddet-mask 遮罩 + .ddet 卡片，同 SettingsPanel / 游客引导弹窗用法），
   portal 到 body —— 不被 .app-shell 的圆角/溢出与视图转场动画上下文裁剪。
   开合动画照 lc-1603（GuestLoginPrompt）：条件挂载没有退场动画，关闭时先挂 `.dsc-mask-closing`
   让遮罩淡出 + 卡片缩下沉，动画走完（170ms）再真卸载。 */
import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { X as IcoXL } from "lucide-react";
import { DISCLAIMER_TEXT, DISCLAIMER_TITLE } from "./DisclaimerText";

export function DisclaimerModal({
  variant,
  onAgree,
  onClose,
  onExit,
}: {
  /** first = 首次启动同意闸门；info = 设置里的只读查看（只有「关闭」） */
  variant: "first" | "info";
  /** first 态点「我已阅读并同意」：App 负责置位 localStorage 并关闭（info 态不需要） */
  onAgree?: () => void;
  /** 关闭：info 态的「关闭」按钮 / Esc / 点遮罩 */
  onClose: () => void;
  /** first 态点「不同意并退出」：App 负责退出应用（info 态不需要） */
  onExit?: () => void;
}) {
  const [closing, setClosing] = useState(false);
  const isFirst = variant === "first";

  // 关闭动画：先挂 closing 类，动画走完再执行真正的动作（对齐 CSS 0.17s）
  const run = (fn: () => void) => {
    if (closing) return;
    setClosing(true);
    window.setTimeout(fn, 170);
  };

  // info 态：Esc 关闭。first 态是同意闸门，不响应 Esc —— 必须显式选择「同意 / 退出」。
  useEffect(() => {
    if (isFirst) return;
    const h = (e: KeyboardEvent) => {
      if (e.key === "Escape") run(onClose);
    };
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
    // run 每次渲染都是新函数，但只读 closing —— 依赖里放 closing / isFirst 语义等价
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [closing, isFirst]);

  return createPortal(
    <div
      className={`ddet-mask${closing ? " dsc-mask-closing" : ""}`}
      onClick={isFirst ? undefined : () => run(onClose)}
    >
      <div className="ddet dsc-modal" onClick={(e) => e.stopPropagation()}>
        {!isFirst && (
          <button className="ddet-close" title="关闭（Esc）" onClick={() => run(onClose)}>
            <IcoXL size={15} strokeWidth={1.8} />
          </button>
        )}
        <header className="ddet-hd">
          <div className="ddet-hdm">
            <h2>{DISCLAIMER_TITLE}</h2>
          </div>
        </header>
        <div className="ddet-body dsc-body">
          <p className="dsc-text">{DISCLAIMER_TEXT}</p>
        </div>
        <div className="dsc-acts">
          {isFirst ? (
            <>
              <button className="dsc-btn ghost" onClick={() => run(() => onExit?.())}>
                不同意并退出
              </button>
              <button className="dsc-btn primary" onClick={() => run(() => onAgree?.())}>
                我已阅读并同意
              </button>
            </>
          ) : (
            <button className="dsc-btn primary" onClick={() => run(onClose)}>
              关闭
            </button>
          )}
        </div>
      </div>
    </div>,
    document.body,
  );
}
