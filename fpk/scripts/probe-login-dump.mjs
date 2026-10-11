// 一次性：登录 fnOS → dump 全部 cookie → 进网关入口 → dump 网关回包。
// 单次运行跑完即退。
import { chromium } from 'playwright';

const BASE = 'http://100.66.1.2:17777';
const UA = 'Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1';

const browser = await chromium.launch({ headless: true });
try {
  const ctx = await browser.newContext({ viewport: { width: 390, height: 844 }, userAgent: UA, hasTouch: true, isMobile: true });
  const page = await ctx.newPage();
  // fnOS 系统登录页（不是 /v/login 影视页——那个只种 Trim-MC-token）
  await page.goto(BASE + '/', { waitUntil: 'domcontentloaded', timeout: 30000 });
  await page.waitForTimeout(3000);
  console.log('landing URL:', page.url());
  if (/login|signin/.test(page.url())) {
    const u = page.locator('#username, input[name="username"], input[type="text"]').first();
    const p = page.locator('#password, input[name="password"], input[type="password"]').first();
    await u.fill('EIKO', { timeout: 8000 });
    await p.fill('Hzh690075', { timeout: 8000 });
    await page.locator('button[type="submit"], button:has-text("登录"), button:has-text("登 录")').first().click();
    await page.waitForTimeout(6000);
    console.log('after login URL:', page.url());
  }
  const cookies = await ctx.cookies();
  console.log('cookies:', cookies.map(c => `${c.name}=${c.value.slice(0, 12)}…(domain=${c.domain})`).join('\n  '));
  // 带全套 cookie 走网关入口
  await page.goto(BASE + '/app/fntvplus/v/', { waitUntil: 'domcontentloaded', timeout: 30000 });
  await page.waitForTimeout(6000);
  console.log('gateway URL:', page.url());
  const txt = (await page.evaluate(() => (document.body?.innerText || '').slice(0, 120)));
  console.log('body text:', txt.replace(/\n/g, ' '));
  const hasPayload = await page.evaluate(() => !!document.querySelector('script[src*="__payload__"], script[src*="fntv-plus."]'));
  console.log('fpk payload injected:', hasPayload);
} finally {
  await browser.close();
}
