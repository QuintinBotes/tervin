/**
 * The terminal font a user picks has to survive leaving the Settings panel.
 *
 * This crosses every layer the workspace has: a keystroke reaches the React
 * store, the store writes through `settings_set` over IPC to SQLite, and the
 * panel is unmounted and rebuilt from that state. A regression anywhere along
 * that path — a dropped IPC command, a store patch that does not merge, a panel
 * that reinitialises from defaults — fails here.
 *
 * Local only: no network, and the profile it writes to is the throwaway HOME
 * that wdio.conf.ts hands the app.
 */
import { Key } from "webdriverio";

const TEST_FONT = '"Tervin E2E Mono", ui-monospace, monospace';

/** Cmd+, on macOS — the `settings.open` binding in ui/src/lib/keymap.ts. */
async function openSettings() {
  await browser.keys([Key.Command, ","]);
  const dialog = await $('[data-testid="settings-dialog"]');
  await dialog.waitForDisplayed({
    timeoutMsg: "Cmd+, did not open the Settings panel",
  });
  return dialog;
}

describe("Settings — terminal font", () => {
  before(async () => {
    // The window paints before the shell and the block database are ready, and a
    // keystroke sent into that gap is dropped. Waiting on the workspace root is
    // the observable signal that the app is listening.
    await $("#root").waitForExist({ timeout: 60_000 });
    await browser.waitUntil(
      async () => Number(await $("#root").getProperty("childElementCount")) > 0,
      { timeout: 60_000, timeoutMsg: "the workspace never rendered" },
    );
  });

  it("applies a typed font to the preview and keeps it after the panel closes", async () => {
    await openSettings();

    // Appearance is the default section, but selecting it explicitly means this
    // test does not start failing the day that default changes.
    await $('[data-testid="settings-nav-appearance"]').click();

    const input = await $('[data-testid="terminal-font-input"]');
    await input.waitForDisplayed();
    await input.setValue(TEST_FONT);

    // The success state a user actually sees: the specimen line below the field
    // is rendered with the font that was typed.
    //
    // Read through `getComputedStyle` rather than `getCSSProperty`, which returns
    // the value through WebdriverIO's CSS parser: that lowercases the family and
    // keeps only the first one, so `"Tervin E2E Mono"` comes back as
    // `tervin e2e mono` and an exact comparison fails on a correct app.
    await browser.waitUntil(
      async () => {
        const family = await browser.execute(() => {
          const el = document.querySelector('[data-testid="terminal-font-preview"]');
          return el ? getComputedStyle(el).fontFamily : "";
        });
        return String(family).includes("Tervin E2E Mono");
      },
      {
        timeout: 10_000,
        timeoutMsg: "the preview never picked up the typed font",
      },
    );

    // Closing unmounts the panel, so reopening rebuilds every field from the
    // workspace state rather than from what is still on screen.
    await $('[data-testid="settings-close-button"]').click();
    await $('[data-testid="settings-dialog"]').waitForDisplayed({
      reverse: true,
      timeoutMsg: "the Settings panel stayed open after Close",
    });

    await openSettings();
    const reopened = await $('[data-testid="terminal-font-input"]');
    await reopened.waitForDisplayed();
    await expect(reopened).toHaveValue(TEST_FONT);
  });
});
