/* [lc-2396] 收藏按钮的状态与写盘逻辑（从 MovieHeroEditorial 搬出，逻辑一字未改）。
 *
 * 为什么提出来：Polaris 详情页（`detail/polaris/`）的头像按钮组里也有收藏键，
 * 两处各写一份 `favorite_get` / `favorite_toggle` + 事件广播，必然出现
 * 「一处点了另一处不亮」。这里做**唯一实现**，两个版式共用。
 */
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export function useFavorite(favItem: Record<string, unknown> | null | undefined) {
  const favGuid = String((favItem?.guid as string) ?? "").trim();
  const [fav, setFav] = useState(false);
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState("");

  useEffect(() => {
    if (!favGuid) return;
    let alive = true;
    invoke<boolean>("favorite_get", { guid: favGuid })
      .then((v) => alive && setFav(!!v))
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [favGuid]);

  const toggle = () => {
    if (!favGuid || busy) return;
    setBusy(true);
    invoke<{ favorited?: boolean }>("favorite_toggle", { item: favItem })
      .then((r) => {
        const on = !!r?.favorited;
        setFav(on);
        setNote(on ? "已收藏，可在侧边栏「收藏」里找到" : "已取消收藏");
        window.dispatchEvent(new Event("anibox-favorites-changed"));
      })
      .catch((e) => setNote(String(e)))
      .finally(() => setBusy(false));
  };

  return { fav, favGuid, busy, note, toggle };
}
