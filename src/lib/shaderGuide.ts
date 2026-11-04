/* [lc-2009] 着色器速查弹窗数据 —— 内容照抄用户提供的折叠块（mpv.config 滤镜指南）。
   一级 4 项：动漫 / 真人 / 滤镜系列速查 / 单个着色器开关。
   组合行的 chips = 可点击应用的预设 key（与 fntv_shader.lua PRESETS 一致）；
   单开关行点击 = 叠加/移除（toggle）。供 ShaderGuideModal 与后续新版 UI 共用。 */

export type GuideChip = { key: string; label: string };
export type GuideRow = {
  scene: string; // 「片源 → 输出」或「菜单名称」
  chips?: GuideChip[]; // 可点击应用的组合
  note?: string; // 推荐组合的纯文字说明（如「无需超分」）
  func?: string; // 功能列（速查表）
  perf: string; // 性能列：低/中/高/极高/—
};
export type GuideGroup = {
  id: string;
  title: string;
  summary: string; // 折叠块 summary 行的小注
  rows: GuideRow[];
};

/** 性能等级 → 颜色（Radix Colors 语义色，见弹窗组件映射） */
export const PERF_LEVELS = ["低", "中", "高", "极高"] as const;

export const ANIME_GUIDE: GuideGroup = {
  id: "anime",
  title: "动漫",
  summary: "4K 片源无需超分；Anime4K 针对动漫优化",
  rows: [
    { scene: "720p → 4K（推荐）", chips: [{ key: "aq_hq_a", label: "Mode A (HQ)" }, { key: "aq_hq_b", label: "Mode B (HQ)" }], perf: "高" },
    { scene: "720p → 4K（性能有限）", chips: [{ key: "aq_ft_a", label: "Mode A (Fast)" }, { key: "aq_ft_b", label: "Mode B (Fast)" }, { key: "aq_ft_c", label: "Mode C (Fast)" }], perf: "中" },
    { scene: "1080p → 4K（推荐）", chips: [{ key: "aq_hq_a", label: "Mode A (HQ)" }, { key: "aq_hq_b", label: "Mode B (HQ)" }], perf: "高 / 中" },
    { scene: "1080p → 4K（性能有限）", chips: [{ key: "aq_ft_a", label: "Mode A (Fast)" }, { key: "aq_ft_b", label: "Mode B (Fast)" }, { key: "aq_ft_c", label: "Mode C (Fast)" }], perf: "中" },
    { scene: "1080p → 4K（效率）", chips: [{ key: "cunny_2x12", label: "CuNNy/2x12" }, { key: "cunny_fast", label: "CuNNy/fast" }], perf: "低" },
    { scene: "1080p → 4K（高开销）", chips: [{ key: "cunny_4x32", label: "CuNNy/4x32" }, { key: "cunny_8x32", label: "CuNNy/8x32" }], perf: "高~极高" },
    { scene: "4K 片源", note: "无需超分", perf: "—" },
  ],
};

export const LIVE_GUIDE: GuideGroup = {
  id: "live",
  title: "真人",
  summary: "4K 片源无需超分；不推荐 Anime4K，易油画感、皮肤纹理丢失",
  rows: [
    { scene: "720p → 4K", chips: [{ key: "fsrcnnx_16", label: "FSRCNNX/2x16+SSim+Krig" }], perf: "高" },
    { scene: "720p → 4K（均衡）", chips: [{ key: "fsrcnnx_8", label: "FSRCNNX/2x8+Krig" }], perf: "中" },
    { scene: "720p → 4K（性能优先）", chips: [{ key: "fsrcnnx_2", label: "FSRCNNX/2x2+Krig" }], perf: "低" },
    { scene: "1080p → 4K", chips: [{ key: "fsrcnnx_8", label: "FSRCNNX/2x8+Krig" }, { key: "fsrcnnx_16", label: "FSRCNNX/2x16+SSim+Krig" }], perf: "中 / 高" },
    { scene: "1080p → 4K（追求质感）", chips: [{ key: "artcnn_32_ravu4", label: "ArtCNN/32+RAVU-LITE-R4" }], perf: "高" },
    { scene: "1080p → 4K（效率）", chips: [{ key: "fsrcnnx_2", label: "FSRCNNX/2x2+Krig" }, { key: "cunny_4x12", label: "CuNNy/4x12" }], perf: "低~中" },
    { scene: "4K 片源", note: "无需超分；不推荐 Anime4K", perf: "—" },
  ],
};

export type SeriesGroup = {
  id: string;
  title: string;
  summary: string;
  rows: GuideRow[];
};

export const SERIES_GROUPS: SeriesGroup[] = [
  {
    id: "anime4k",
    title: "3.1 Anime4K 系列",
    summary: "真人易涂抹/塑料感，不建议用于真人",
    rows: [
      { scene: "Mode A (HQ)", chips: [{ key: "aq_hq_a", label: "应用" }], func: "Clamp_Highlights + Restore_CNN_VL + Upscale_CNN_x2_VL + AutoDownscalePre_x2/x4 + Upscale_CNN_x2_M", perf: "高" },
      { scene: "Mode B (HQ)", chips: [{ key: "aq_hq_b", label: "应用" }], func: "同上，Restore_CNN_Soft_VL", perf: "高" },
      { scene: "Mode C (HQ)", chips: [{ key: "aq_hq_c", label: "应用" }], func: "Clamp_Highlights + Upscale_Denoise_CNN_x2_VL + AutoDownscalePre_x2/x4 + Upscale_CNN_x2_M", perf: "高" },
      { scene: "Mode A+A (HQ)", chips: [{ key: "aq_hq_aa", label: "应用" }], func: "在 Mode A 基础上多一层 Restore_CNN_M（二次重建）", perf: "极高" },
      { scene: "Mode B+B (HQ)", chips: [{ key: "aq_hq_bb", label: "应用" }], func: "在 Mode B 基础上多一层 Restore_CNN_Soft_M（二次柔和重建）", perf: "极高" },
      { scene: "Mode C+A (HQ)", chips: [{ key: "aq_hq_ca", label: "应用" }], func: "降噪放大 + 额外重建", perf: "极高" },
      { scene: "Mode A (Fast)", chips: [{ key: "aq_ft_a", label: "应用" }], func: "Clamp_Highlights + Restore_CNN_M + Upscale_CNN_x2_M + AutoDownscalePre_x2/x4 + Upscale_CNN_x2_S", perf: "中" },
      { scene: "Mode B (Fast)", chips: [{ key: "aq_ft_b", label: "应用" }], func: "同上，Restore_CNN_Soft_M", perf: "中" },
      { scene: "Mode C (Fast)", chips: [{ key: "aq_ft_c", label: "应用" }], func: "Clamp_Highlights + Upscale_Denoise_CNN_x2_M + AutoDownscalePre_x2/x4 + Upscale_CNN_x2_S", perf: "中" },
      { scene: "Mode A+A (Fast)", chips: [{ key: "aq_ft_aa", label: "应用" }], func: "Clamp_Highlights + Restore_CNN_M + Upscale_CNN_x2_M + Restore_CNN_S + AutoDownscalePre_x2/x4 + Upscale_CNN_x2_S", perf: "中" },
      { scene: "Mode B+B (Fast)", chips: [{ key: "aq_ft_bb", label: "应用" }], func: "Clamp_Highlights + Restore_CNN_Soft_M + Upscale_CNN_x2_M + AutoDownscalePre_x2/x4 + Restore_CNN_Soft_S + Upscale_CNN_x2_S", perf: "中" },
      { scene: "Mode C+A (Fast)", chips: [{ key: "aq_ft_ca", label: "应用" }], func: "Clamp_Highlights + Upscale_Denoise_CNN_x2_M + AutoDownscalePre_x2/x4 + Restore_CNN_S + Upscale_CNN_x2_S", perf: "中" },
      { scene: "A4K-DTDD", chips: [{ key: "aq_dtdd", label: "应用" }], func: "Denoise_Bilateral_Mode + Deblur_DoG + Thin_HQ + Darken_HQ（去噪、去模糊、细化、加深）", perf: "中" },
    ],
  },
  {
    id: "fsrcnnx",
    title: "3.2 FSRCNNX 系列",
    summary: "通用型 CNN，真人首选，自带 Krig 色度放大",
    rows: [
      { scene: "FSRCNNX/2x2+Krig", chips: [{ key: "fsrcnnx_2", label: "应用" }], func: "FSRCNNX_x2_2_12_0_4_1_v1_fastv2 + KrigBilateral（轻量亮度 + 色度）", perf: "低" },
      { scene: "FSRCNNX/2x8+Krig", chips: [{ key: "fsrcnnx_8", label: "应用" }], func: "FSRCNNX_x2_8_0_4_1 + KrigBilateral（中等亮度 + 色度）", perf: "中" },
      { scene: "FSRCNNX/2x16+SSim+Krig", chips: [{ key: "fsrcnnx_16", label: "应用" }], func: "FSRCNNX_x2_16_0_4_1 + SSimSuperRes_RT + KrigBilateral（高质量 + 后处理修正 + 色度）", perf: "高" },
      { scene: "FSRCNNX/2x56+SSim+Krig", chips: [{ key: "fsrcnnx_56", label: "应用" }], func: "FSRCNNX_x2_56_16_4_1 + SSimSuperRes_RT + KrigBilateral（极致质量，高配置）", perf: "极高" },
    ],
  },
  {
    id: "artcnn",
    title: "3.3 ArtCNN 系列",
    summary: "新版不含 CfL，需手动开 CHROMA；_DS 降噪锐化、_DN 强降噪；C4F8 质量最好但开销极高，C4F16/C4F32 为常用",
    rows: [
      { scene: "ArtCNN/16", chips: [{ key: "artcnn_16", label: "应用" }], func: "ArtCNN_C4F16（中等质量亮度放大）", perf: "中" },
      { scene: "ArtCNN/16DS", chips: [{ key: "artcnn_16ds", label: "应用" }], func: "ArtCNN_C4F16_DS（降噪 + 中等）", perf: "中" },
      { scene: "ArtCNN/16DN", chips: [{ key: "artcnn_16dn", label: "应用" }], func: "ArtCNN_C4F16_DN（强降噪 + 中等）", perf: "中" },
      { scene: "ArtCNN/32", chips: [{ key: "artcnn_32", label: "应用" }], func: "ArtCNN_C4F32（高质量亮度放大）", perf: "高" },
      { scene: "ArtCNN/32DS", chips: [{ key: "artcnn_32ds", label: "应用" }], func: "ArtCNN_C4F32_DS（降噪 + 高质量）", perf: "高" },
      { scene: "ArtCNN/32DN", chips: [{ key: "artcnn_32dn", label: "应用" }], func: "ArtCNN_C4F32_DN（强降噪 + 高质量）", perf: "高" },
      { scene: "ArtCNN/48", chips: [{ key: "artcnn_48", label: "应用" }], func: "ArtCNN_v0_C4F8（极高质量·更大模型）", perf: "极高" },
      { scene: "ArtCNN/48DS", chips: [{ key: "artcnn_48ds", label: "应用" }], func: "ArtCNN_v0_C4F8_DS（降噪 + 极高质量）", perf: "极高" },
      { scene: "ArtCNN/48DS_CMP", chips: [{ key: "artcnn_48ds_cmp", label: "应用" }], func: "ArtCNN_v0_C4F8_DS_CMP（+压缩伪影修复）", perf: "极高" },
    ],
  },
  {
    id: "cunny",
    title: "3.4 CuNNy 系列",
    summary: "新版不含 CfL，需手动配色度；主要针对动漫/视觉小说，真人可能不如 FSRCNNX/ArtCNN；dp4a 需 Vulkan + dp4a 硬件",
    rows: [
      { scene: "CuNNy/faster", chips: [{ key: "cunny_faster", label: "应用" }], func: "CuNNy-faster-DS（极轻量放大）", perf: "低" },
      { scene: "CuNNy/fast", chips: [{ key: "cunny_fast", label: "应用" }], func: "CuNNy-fast-DS（轻量放大）", perf: "低" },
      { scene: "CuNNy/2x12", chips: [{ key: "cunny_2x12", label: "应用" }], func: "CuNNy-2x12-DS（入门放大）", perf: "低" },
      { scene: "CuNNy/3x12", chips: [{ key: "cunny_3x12", label: "应用" }], func: "CuNNy-3x12-DS（低端放大）", perf: "中" },
      { scene: "CuNNy/4x12", chips: [{ key: "cunny_4x12", label: "应用" }], func: "CuNNy-4x12-DS（中低端放大）", perf: "中" },
      { scene: "CuNNy/4x16", chips: [{ key: "cunny_4x16", label: "应用" }], func: "CuNNy-4x16-DS（均衡放大）", perf: "中" },
      { scene: "CuNNy/4x24", chips: [{ key: "cunny_4x24", label: "应用" }], func: "CuNNy-4x24-DS（中高端放大）", perf: "高" },
      { scene: "CuNNy/4x32", chips: [{ key: "cunny_4x32", label: "应用" }], func: "CuNNy-4x32-DS（高端放大）", perf: "高" },
      { scene: "CuNNy/8x32", chips: [{ key: "cunny_8x32", label: "应用" }], func: "CuNNy-8x32-DS（极致放大）", perf: "极高" },
      { scene: "dp4a 版本", note: "同规格文件名带 -Q，Vulkan 加速，需 mpv.conf 设 vo=gpu-next + gpu-api=vulkan", perf: "比同规格低一档" },
    ],
  },
];

export const SWITCH_GROUP: GuideGroup = {
  id: "switch",
  title: "单个着色器开关",
  summary: "点击叠加到当前组合 / 再点移除；CHROMA: CFL / Krig 需手动开才有色度放大",
  rows: [
    { scene: "LUMA: NNEDI3/x4", chips: [{ key: "sw_nnedi3_x4", label: "叠加" }], func: "nnedi3_nns128_win8x4（边缘导向亮度放大）", perf: "中" },
    { scene: "LUMA: NNEDI3/x6", chips: [{ key: "sw_nnedi3_x6", label: "叠加" }], func: "nnedi3_nns128_win8x6（更高质量）", perf: "高" },
    { scene: "LUMA: FSRCNNX/2x16", chips: [{ key: "sw_fsrcnnx_16", label: "叠加" }], func: "FSRCNNX_x2_16_0_4_1（亮度放大）", perf: "高" },
    { scene: "LUMA: RAVU-ZOOM-R2", chips: [{ key: "sw_ravu_r2", label: "叠加" }], func: "ravu_zoom_r2（亮度缩放）", perf: "中" },
    { scene: "LUMA: RAVU-ZOOM-R3", chips: [{ key: "sw_ravu_r3", label: "叠加" }], func: "ravu-zoom-ar-r3.hook（更高质量）", perf: "中" },
    { scene: "LUMA: RAVU-LITE-R4", chips: [{ key: "sw_ravu_r4", label: "叠加" }], func: "ravu-lite-ar-r4.hook（轻量）", perf: "低" },
    { scene: "SSimSuperRes_RT", chips: [{ key: "sw_ssim_sr", label: "叠加" }], func: "SSimSuperRes_RT（后处理锐化修正）", perf: "中" },
    { scene: "SSimDownscaler_RT", chips: [{ key: "sw_ssim_ds", label: "叠加" }], func: "SSimDownscaler_RT（下采样，高→低）", perf: "中" },
    { scene: "CHROMA: CFL_RT", chips: [{ key: "sw_cfl", label: "叠加" }], func: "CfL_Prediction_RT（色度放大）", perf: "低" },
    { scene: "CHROMA: Krig", chips: [{ key: "sw_krig", label: "叠加" }], func: "KrigBilateral（色度放大）", perf: "低" },
    { scene: "AMD: FSR_EASU_RT", chips: [{ key: "sw_fsr_easu", label: "叠加" }], func: "AMD_FSR_EASU_rgb_RT（FSR 放大 EASU）", perf: "低" },
    { scene: "AMD: FSR_RGB_RT", chips: [{ key: "sw_fsr_rgb", label: "叠加" }], func: "AMD_FSR_rgb_RT（FSR 放大 RGB）", perf: "低" },
    { scene: "Anime4K: 重建", chips: [{ key: "sw_aq_restore", label: "叠加" }], func: "Anime4K_Restore_CNN_M（线条重建）", perf: "中" },
    { scene: "Anime4K: 放大", chips: [{ key: "sw_aq_gan", label: "叠加" }], func: "Anime4K_Upscale_GAN_x2_M（GAN 放大）", perf: "高" },
    { scene: "OUTPUT: 自适应锐化_RT", chips: [{ key: "sw_sharpen", label: "叠加" }], func: "adaptive_sharpen_RT（自适应锐化）", perf: "低" },
  ],
};

/** 通用建议（弹窗底部固定提示，照抄 mpv.config「通用建议」要点） */
export const GUIDE_TIPS = [
  "色度放大优先：CfL 或 KrigBilateral 叠加在链首，提升色彩边缘。",
  "亮度放大只选一个：ArtCNN 与 RAVU 等亮度放大不要同时叠加。",
  "后处理放在放大之后：SSimSuperRes、自适应锐化用于修正放大后的伪影。",
  "下采样仅用于高→低：SSimDownscaler 适合 4K 片源在 1080p 屏播放。",
  "4K 片源通常无需超分；低码率噪点多的片源适合恢复/降噪路线而非强力超分。",
];
