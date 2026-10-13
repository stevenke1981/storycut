// Use the isolated QA runner; no changes to the app's production dependencies.
const { chromium } = require('../../target/ui-export-evidence/runner/node_modules/playwright-core');
const fs = require('node:fs');
const path = require('node:path');

(async () => {
  const root = path.resolve(__dirname, '../..');
  const report = JSON.parse(fs.readFileSync(process.argv[2], 'utf8'));
  const evidence = path.join(root, 'target/ui-export-evidence');
  const browser = await chromium.launch({ executablePath: 'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe', headless: true });
  const page = await browser.newPage({ viewport: { width:1600, height:1100 }, deviceScaleFactor:1 });
  const errors=[];
  page.on('pageerror', error => errors.push(error.message));
  try {
    await page.goto('http://127.0.0.1:14929');
    await page.screenshot({path:path.join(evidence,'initial.png'),fullPage:true});
    await page.getByRole('button', {name:'開啟',exact:true}).click();
    await page.getByRole('button', {name:'開始匯出',exact:true}).waitFor({state:'visible'});
    await page.waitForFunction(() => !document.querySelector('.render-button')?.disabled);
    await page.getByRole('button', {name:'開始匯出',exact:true}).click();
    await page.waitForFunction(() => document.querySelector('.job-state')?.textContent === 'running' && document.querySelector('.job-actions span')?.textContent?.includes('%'));
    await page.getByRole('button',{name:'產生精確影格',exact:true}).click();
    await page.locator('.frame-preview').waitFor({state:'visible'});
    await page.waitForFunction(() => document.querySelector('.job-state')?.textContent === 'running');
    if (!await page.getByRole('button',{name:'開始匯出',exact:true}).isDisabled()) throw new Error('Export button must prevent a second GUI submit');
    await page.locator('.job-card').scrollIntoViewIfNeeded();
    await page.screenshot({path:path.join(evidence,'running.png'),fullPage:true});
    await page.getByRole('button',{name:'取消',exact:true}).click();
    await page.waitForFunction(() => document.querySelector('.job-state')?.textContent === 'cancelled');
    await page.screenshot({path:path.join(evidence,'cancelled.png'),fullPage:true});
    const track = page.getByRole('textbox',{name:'輸出工作 ID'});
    await track.fill(report.succeeded_job.job_id);
    await page.getByRole('button',{name:'追蹤工作',exact:true}).click();
    await page.waitForFunction(() => document.querySelector('.job-state')?.textContent === 'succeeded');
    if (!await page.locator('.job-card').getByText('success.mp4',{exact:true}).isVisible()) throw new Error('Verified output path is missing');
    await page.screenshot({path:path.join(evidence,'succeeded.png'),fullPage:true});
    await page.reload();
    await page.getByRole('button',{name:'開啟',exact:true}).click();
    await page.waitForFunction(() => document.querySelector('.job-state')?.textContent === 'succeeded');
    await page.getByRole('textbox',{name:'輸出工作 ID'}).fill(report.failed_job.job_id);
    await page.getByRole('button',{name:'追蹤工作',exact:true}).click();
    await page.waitForFunction(() => document.querySelector('.job-state')?.textContent === 'failed');
    await page.screenshot({path:path.join(evidence,'failed.png'),fullPage:true});
    if(errors.length) throw new Error(errors.join('\n'));
    fs.writeFileSync(path.join(evidence,'report.json'),JSON.stringify({status:'PASS',backend:'real CLI/workspace daemon',transport:'QA HTTP replaces Tauri IPC and picker only',browser:await browser.version(),states:['running','cancelled','succeeded','failed'],persisted_tracking:true,console_errors:errors,native_UI_verified:false},null,2));
    console.log('UI export states PASS: '+evidence);
  } catch (error) {
    await page.screenshot({path:path.join(evidence,'error.png'),fullPage:true});
    console.error('Browser errors:',errors);
    throw error;
  } finally { await browser.close(); }
})().catch(error => {console.error(error);process.exitCode=1;});
