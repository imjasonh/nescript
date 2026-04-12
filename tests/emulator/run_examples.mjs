// Drive the local jsnes harness from puppeteer to sanity-check every compiled
// example ROM. For each ROM we load it, run a couple of seconds of frames,
// capture a screenshot, and record basic "did it render" stats. This is a
// load+render smoke test, not a gameplay test.

import { promises as fs } from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import puppeteer from "puppeteer";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(__dirname, "..", "..");
const examplesDir = path.join(repoRoot, "examples");
const screenshotsDir = path.join(__dirname, "screenshots");
const harnessUrl = pathToFileURL(path.join(__dirname, "harness.html")).toString();

const FRAMES_TO_RUN = 180; // ~3 seconds at 60 fps, enough to get past a title/boot
const SCREENSHOT_FRAME = 180;

async function listRoms() {
  const entries = await fs.readdir(examplesDir);
  return entries
    .filter((f) => f.endsWith(".nes"))
    .sort()
    .map((f) => ({ name: f.replace(/\.nes$/, ""), file: path.join(examplesDir, f) }));
}

async function main() {
  await fs.mkdir(screenshotsDir, { recursive: true });
  const roms = await listRoms();
  if (roms.length === 0) {
    console.error("no .nes files found in examples/ — build them first");
    process.exit(1);
  }

  const browser = await puppeteer.launch({
    headless: "new",
    args: ["--no-sandbox", "--disable-setuid-sandbox", "--allow-file-access-from-files"],
  });

  const results = [];
  let failures = 0;

  try {
    for (const rom of roms) {
      const page = await browser.newPage();
      const consoleErrors = [];
      page.on("pageerror", (err) => consoleErrors.push(String(err)));
      page.on("console", (msg) => {
        if (msg.type() === "error") consoleErrors.push(msg.text());
      });

      await page.goto(harnessUrl, { waitUntil: "load" });
      // Wait until the harness reports ready.
      await page.waitForFunction("window.nesHarness && document.getElementById('info').textContent === 'ready'");

      const romBytes = await fs.readFile(rom.file);
      const romB64 = romBytes.toString("base64");

      let booted = true;
      let bootError = null;
      try {
        await page.evaluate((b64) => window.nesHarness.loadRomBase64(b64), romB64);
      } catch (err) {
        booted = false;
        bootError = String(err);
      }

      // Collect hashes across frames so we can detect a frozen / all-black boot.
      const frameHashes = [];
      if (booted) {
        try {
          for (let i = 0; i < FRAMES_TO_RUN; i++) {
            await page.evaluate(() => window.nesHarness.frame());
            if (i === 29 || i === 89 || i === 149 || i === SCREENSHOT_FRAME - 1) {
              const stats = await page.evaluate(() => window.nesHarness.frameStats());
              frameHashes.push({ frame: i + 1, ...stats });
            }
          }
        } catch (err) {
          booted = false;
          bootError = String(err);
        }
      }

      const screenshotPath = path.join(screenshotsDir, `${rom.name}.png`);
      if (booted) {
        const canvas = await page.$("#screen");
        await canvas.screenshot({ path: screenshotPath });
      }

      const lastStats = frameHashes[frameHashes.length - 1];
      const firstStats = frameHashes[0];
      const uniqueHashes = new Set(frameHashes.map((h) => h.hash)).size;
      const rendered = booted && lastStats && lastStats.nonBlack > 0;
      const animated = uniqueHashes > 1;

      const status = rendered ? "OK" : "FAIL";
      if (!rendered) failures++;

      results.push({
        name: rom.name,
        status,
        bootError,
        rendered,
        animated,
        frames: frameHashes,
        consoleErrors,
        screenshot: booted ? path.relative(repoRoot, screenshotPath) : null,
      });

      console.log(
        `${status.padEnd(4)} ${rom.name.padEnd(28)} ` +
          (rendered
            ? `nonBlack=${lastStats.nonBlack}/${lastStats.totalPixels} uniqueHashes=${uniqueHashes} animated=${animated}`
            : `boot=${booted} bootError=${bootError ?? "none"}`),
      );
      if (consoleErrors.length > 0) {
        for (const e of consoleErrors) console.log("    console:", e);
      }

      await page.close();
    }
  } finally {
    await browser.close();
  }

  const reportPath = path.join(__dirname, "report.json");
  await fs.writeFile(reportPath, JSON.stringify({ generatedAt: new Date().toISOString(), results }, null, 2));
  console.log(`\nreport written to ${path.relative(repoRoot, reportPath)}`);
  console.log(`${results.length - failures}/${results.length} ROMs rendered successfully`);

  if (failures > 0) process.exit(1);
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
