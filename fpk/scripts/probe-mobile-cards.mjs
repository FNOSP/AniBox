// [lc-1306] 一次性实测：fnOS 登录 → 网关入口 → 量首页卡片真实几何。
// 单次运行跑完即退（用户 CPU 纪律）。带网络监听定位登录失败原因。
import { chromium } from 'playwright';

const BASE = 'http://100.66.1.2:17777';
const ENTRY = BASE + '/app/fntvplus/v/';
const UA = 'Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1';

const browser = await chromium.launch({ headless: true });
try {
  const ctx = await browser.newContext({
    viewport: { width: 390, height: 844 },
    userAgent: UA,
    hasTouch: true,
    isMobile: true,
    deviceScaleFactor: 2,
  });
  const page = await ctx.newPage();
  // 网络监听：只记录 login/auth 相关请求（登录失败定位用）
  page.on('response', (r) => {
    const u = r.url();
    if (/login|auth|session/i.test(u) && !/\.(js|css|png|svg)/.test(u)) {
      r.text().then(t => console.log('  [net]', r.status(), u.slice(BASE.length).slice(0, 60), t.slice(0, 150).replace(/\n/g, ' '))).catch(() => {});
    }
  });

  // ① fnOS 系统登录（/login 是 SSO 页，/v/login 是影视页——网关要前者）
  await page.goto(BASE + '/login', { waitUntil: 'domcontentloaded', timeout: 30000 });
  await page.waitForTimeout(3500);
  const u = page.locator('input#username, input[name="username"]').first();
  await u.click();
  await u.pressSequentially('EIKO', { delay: 70 });
  const p = page.locator('input#password, input[name="password"]').first();
  await p.click();
  await p.pressSequentially('Hzh690075', { delay: 70 });
  await page.waitForTimeout(500);
  const btn = page.locator('button[type="submit"], button:has-text("登录")').first();
  console.log('  按钮禁用态:', await btn.isDisabled().catch(() => 'n/a'));
  await btn.click();
  // 等离开 /login（最多 15s）
  let left = false;
  for (let i = 0; i < 15; i++) {
    await page.waitForTimeout(1000);
    if (!/\/login/.test(page.url())) { left = true; break; }
  }
  console.log('  登录后 URL:', page.url(), left ? '(已离开)' : '(未离开)');
  const cookies = await ctx.cookies();
  console.log('  cookies:', cookies.map(c => c.name).join(', ') || '(none)');

  // ② 网关入口
  await page.goto(ENTRY, { waitUntil: 'domcontentloaded', timeout: 30000 });
  // ③ fnOS SPA 卡加载门（memory 必杀技：循环 reload，上限 8 次有界）
  for (let i = 0; i < 8; i++) {
    const txt = await page.evaluate(() => (document.body?.innerText || '').slice(0, 300));
    if (!txt.includes('加载中') && !txt.includes('invalid token')) break;
    await page.reload({ waitUntil: 'domcontentloaded' });
    await page.waitForTimeout(4000);
  }
  await page.waitForTimeout(20000);

  // ④ 量卡片几何
  const data = await page.evaluate(() => {
    const vw = window.innerWidth, vh = window.innerHeight;
    const out = {
      url: location.href.slice(0, 90), vw, vh, dpr: window.devicePixelRatio,
      flags: [...document.documentElement.classList].filter(c => c.startsWith('fnos-')),
      hasPayload: !!document.querySelector('script[src*="fntv-plus"], script[src*="__payload__"]'),
      carousel: null, rows: [], pageHeight: document.documentElement.scrollHeight,
    };
    const car = document.querySelector('[data-fntv-carousel-style]');
    if (car) {
      const r = car.getBoundingClientRect();
      out.carousel = { style: car.getAttribute('data-fntv-carousel-style'), w: Math.round(r.width), h: Math.round(r.height) };
    }
    // 卡片行：向上找含 >=3 张 card-root 的横向容器
    const seen = new Set();
    document.querySelectorAll('[class*="card-root"]').forEach((card) => {
      const row = card.parentElement;
      if (!row || seen.has(row)) return;
      const cards = row.querySelectorAll('[class*="card-root"]');
      if (cards.length < 2) return;
      seen.add(row);
      const r0 = cards[0].getBoundingClientRect();
      const rr = row.getBoundingClientRect();
      const rcs = getComputedStyle(row);
      const ccs = getComputedStyle(cards[0]);
      // 行标题（向上 4 层找标题）
      let head = row.parentElement, title = '';
      for (let i = 0; i < 4 && head && !title; i++) {
        const h = head.querySelector('strong,h2,h3,[class*="title"]');
        if (h) title = (h.textContent || '').trim().slice(0, 20);
        head = head.parentElement;
      }
      // 可见张数
      let visible = 0;
      cards.forEach(c => { const cr = c.getBoundingClientRect(); if (cr.left < vw && cr.right > 0) visible++; });
      out.rows.push({
        title, cards: cards.length, visible,
        cardW: Math.round(r0.width), cardH: Math.round(r0.height),
        ar: (r0.width / Math.max(1, r0.height)).toFixed(2),
        rowW: Math.round(rr.width), rowLeft: Math.round(rr.left),
        gap: rcs.gap, display: rcs.display, overflow: rcs.overflowX,
        cardWidthCSS: ccs.width, cardMinWidth: ccs.minWidth, cardFlex: ccs.flex,
      });
    });
    return out;
  });
  console.log(JSON.stringify(data, null, 1));
  await page.screenshot({ path: 'C:/Users/24305/AppData/Local/Temp/fpk-home-mobile.png' });
  console.log('screenshot saved');
} finally {
  await browser.close();
}
