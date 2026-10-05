// Runs before every latchkey the backend starts, from the `node` that
// run_e2e_auth.sh stages in the test runtime. Exit 0 lets the real
// latchkey run, HANDLED means this script already did the command's
// work, anything else is the command's failure.
//
// Three jobs:
// - log the run, so a spec can say what ran before the person clicked;
// - refuse a run that would open the real keychain — latchkey reads it
//   at startup unless it has a key or a gateway, so a leak here would
//   prompt the person running the tests;
// - after the real `ensure-browser` has found a browser, wrap it: same
//   binary, but headless (latchkey hard-codes a window) and with every
//   hostname resolved to the fake internet. The discovery is latchkey's
//   own and runs only when the backend asks for it, so a login that
//   skips `ensure-browser` fails here as it would for a person.
import { spawnSync } from "node:child_process";
import { appendFileSync, chmodSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";

const HANDLED = 100;
const args = process.argv.slice(3);
const env = process.env;
const gateway = Boolean(env.LATCHKEY_GATEWAY);

if (env.DATALIB_TEST_AUTH_SPY_LOG) {
  appendFileSync(env.DATALIB_TEST_AUTH_SPY_LOG, JSON.stringify({ args, gateway }) + "\n");
}

if (!env.LATCHKEY_ENCRYPTION_KEY && !gateway) {
  process.stderr.write(
    "e2e_auth: refusing to run latchkey with neither LATCHKEY_ENCRYPTION_KEY nor " +
      "LATCHKEY_GATEWAY set; it would open the real keychain.\n",
  );
  process.exit(97);
}

if (args.includes("ensure-browser") && !gateway) {
  if (env.DATALIB_TEST_AUTH_NO_BROWSER) {
    // The one stand-in: a machine with no browser to find. latchkey's
    // own words for it.
    process.stderr.write(
      "Error: No browser found after trying sources: existing-config, system-browser, " +
        "existing-playwright-browser\n",
    );
    process.exit(1);
  }
  const real = spawnSync(process.execPath, process.argv.slice(2), { stdio: "inherit" });
  if (real.status !== 0) process.exit(real.status ?? 1);
  const file = path.join(env.LATCHKEY_DIRECTORY, "config.json");
  const config = JSON.parse(readFileSync(file, "utf8"));
  const found = config.browser.executablePath;
  const wrapper = env.DATALIB_TEST_AUTH_BROWSER_WRAPPER;
  // A second login finds the wrapper itself under `existing-config`.
  if (found === wrapper) process.exit(HANDLED);
  const quote = (s) => `'${String(s).replace(/'/g, `'\\''`)}'`;
  writeFileSync(
    wrapper,
    `#!/bin/sh\nexec ${quote(found)} --headless=new ` +
      `--host-resolver-rules=${quote(`MAP * 127.0.0.1:${env.DATALIB_TEST_AUTH_FAKE_PORT}`)} ` +
      `--ignore-certificate-errors "$@"\n`,
  );
  chmodSync(wrapper, 0o755);
  writeFileSync(`${wrapper}.found`, found);
  config.browser = { ...config.browser, executablePath: wrapper };
  writeFileSync(file, JSON.stringify(config, null, 2), { mode: 0o600 });
  process.exit(HANDLED);
}
