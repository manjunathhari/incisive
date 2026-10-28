// Loads the app in headless Chromium, records console errors and page errors,
// types a question, and reports whether hydration made the page interactive.
const { chromium } = require('playwright');
(async () => {
  const b = await chromium.launch();
  const ctx = await b.newContext({ extraHTTPHeaders: { 'x-dev-user-email': 'owner@example.com' } });
  const p = await ctx.newPage();
  const out = [];
  p.on('console', m => out.push(`console.${m.type()}: ${m.text()}`));
  p.on('pageerror', e => out.push(`pageerror: ${e.message}\n${e.stack || ''}`));
  p.on('requestfailed', r => out.push(`requestfailed: ${r.url()} ${r.failure() && r.failure().errorText}`));
  p.on('response', r => { if (r.status() >= 400) out.push(`http ${r.status()}: ${r.url()}`); });
  await p.goto('http://localhost:8080/', { waitUntil: 'networkidle' });
  await p.waitForTimeout(3000);
  await p.fill('#question', 'Does robotic inguinal hernia repair reduce pain?');
  await p.waitForTimeout(1000);
  const disabled = await p.$eval('button[type=submit]', b => b.disabled);
  out.push(`send_button_disabled_after_typing=${disabled}`);
  const html = await p.content();
  out.push(`scripts: ${(html.match(/<script[^>]*>/g) || []).join(' ')}`.slice(0, 600));
  out.push(`links: ${(html.match(/<link[^>]*>/g) || []).join(' ')}`.slice(0, 600));
  console.log(out.join('\n'));
  await b.close();
})();
