// Loads the app in headless Chromium and reports console errors, page errors,
// failed requests and whether the Send button wakes up after typing.
const { chromium } = require('playwright');
const out = [];
const log = s => { out.push(s); console.log(s); };
(async () => {
  let b;
  try {
    b = await chromium.launch();
    const ctx = await b.newContext({ extraHTTPHeaders: { 'x-dev-user-email': 'owner@example.com' } });
    const p = await ctx.newPage();
    p.on('console', m => log(`console.${m.type()}: ${m.text()}`.slice(0, 900)));
    p.on('pageerror', e => log(`pageerror: ${e.message} | ${(e.stack || '').slice(0, 700)}`));
    p.on('requestfailed', r => log(`requestfailed: ${r.url()} ${r.failure() && r.failure().errorText}`));
    p.on('response', r => { if (r.status() >= 400) log(`http ${r.status()}: ${r.url()}`); });
    const resp = await p.goto('http://localhost:8080/', { waitUntil: 'load', timeout: 30000 });
    log(`goto status=${resp && resp.status()}`);
    await p.waitForTimeout(5000);
    const html = await p.content();
    log(`html_len=${html.length} has_textarea=${html.includes('id="question"')}`);
    log(`scripts: ${(html.match(/<script[^>]*>/g) || []).join(' ')}`.slice(0, 700));
    log(`links: ${(html.match(/<link[^>]*>/g) || []).join(' ')}`.slice(0, 700));
    log(`body_start: ${html.slice(html.indexOf('<body'), html.indexOf('<body') + 600)}`);
    try {
      await p.fill('#question', 'Does robotic inguinal hernia repair reduce pain?', { timeout: 5000 });
      await p.waitForTimeout(1500);
      log(`send_disabled_after_typing=${await p.$eval('button[type=submit]', x => x.disabled)}`);
    } catch (e) { log(`fill_error: ${e.message}`.slice(0, 500)); }
  } catch (e) {
    log(`probe_error: ${e.message}`.slice(0, 800));
  } finally {
    if (b) await b.close();
    require('fs').writeFileSync('probe.log', out.join('\n') + '\n');
  }
})();
