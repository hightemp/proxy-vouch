import { test, expect, type Page } from "@playwright/test";
import { defaultSettings, type ExportOptions, type Row } from "../src/types";

async function previewWithRows(page: Page) {
  const rows: Row[] = ["DE", "US", null].map((countryCode, index) => ({
    id: index + 1,
    address: `proxy${index + 1}.example:8080`,
    host: `proxy${index + 1}.example`,
    port: 8080,
    username: "",
    hasCredentials: false,
    requestedProtocol: index < 2 ? "http" : "auto",
    protocol: index < 2 ? "http" : "auto",
    status: index < 2 ? "Working" : "Unchecked",
    label: "",
    source: "Pasted text",
    line: index + 1,
    error: null,
    result:
      index < 2
        ? {
            status: "Working",
            detected: "http",
            authentication: "Not required",
            latencyMs: 25,
            totalDurationMs: 35,
            exitIp: `198.51.100.${index + 1}`,
            countryCode,
            checkedAt: "2026-09-13T10:00:00Z",
            code: "",
            stage: "complete",
            message: "The check request completed successfully.",
            checkUrl: defaultSettings.url,
            attempts: [],
          }
        : null,
  }));
  await page.addInitScript(
    ({ rows, settings }) => {
      const calls: { command: string; args: unknown }[] = [];
      Object.assign(window, {
        isTauri: true,
        testCalls: calls,
        __TAURI_INTERNALS__: {
          metadata: { currentWindow: { label: "main" } },
          transformCallback: () => 1,
          invoke: async (command: string, args: Record<string, unknown>) => {
            calls.push({ command, args });
            if (command === "snapshot")
              return {
                revision: 1,
                reset: true,
                rows,
                running: false,
                runId: 1,
                scheduled: 2,
                completed: 2,
                total: rows.length,
                counts: { Working: 2, Unchecked: 1 },
              };
            if (command === "load_preferences")
              return JSON.parse(
                localStorage.getItem("test-preferences") ??
                  JSON.stringify({ theme: "light", check: settings }),
              );
            if (command === "save_preferences") {
              localStorage.setItem(
                "test-preferences",
                JSON.stringify(args.preferences),
              );
              return null;
            }
            if (command === "storage_status")
              return {
                directory: "/test",
                savedRevision: 1,
                error: null,
                notice: null,
              };
            if (command === "export_data")
              return (args.options as { ids: number[] }).ids.length;
            return null;
          },
        },
      });
    },
    { rows, settings: defaultSettings },
  );
  await page.goto("/");
  await expect(page.locator(".proxy-row")).toHaveCount(3);
}

test("countries and selected copy work with sorting and hidden selections", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1000, height: 650 });
  await previewWithRows(page);
  await expect(
    page.locator(".proxy-row").first().getByLabel("Germany (DE)"),
  ).toHaveText("🇩🇪DE");
  await expect(
    page.locator(".proxy-row").nth(2).locator(".country"),
  ).toHaveText("—");
  await page.getByLabel("Select proxy1.example:8080", { exact: true }).check();
  await page.getByLabel("Select proxy2.example:8080", { exact: true }).check();
  await page.getByLabel("Sort proxies").selectOption("country");
  const layout = await page
    .locator(".proxy-row")
    .first()
    .evaluate((row) => ({
      rowRight: row.getBoundingClientRect().right,
      lastRight: row.lastElementChild!.getBoundingClientRect().right,
      viewport: innerWidth,
      content: document.documentElement.scrollWidth,
    }));
  expect(layout.lastRight).toBeLessThanOrEqual(layout.rowRight);
  expect(layout.content).toBeLessThanOrEqual(layout.viewport);
  await page.screenshot({
    path: "artifacts/country-selection-1000.png",
    fullPage: true,
  });
  await page.getByLabel("Search proxies").fill("US");
  await expect(page.locator(".proxy-row")).toHaveCount(1);
  await page
    .getByRole("button", { name: "Copy selected (2)", exact: true })
    .click();
  await expect(page.getByText("2 records copied to clipboard.")).toBeVisible();
  const calls = await page.evaluate<{ args: { options: ExportOptions } }[]>(
    "window.testCalls.filter(call => call.command === 'export_data')",
  );
  expect(calls).toEqual([
    {
      command: "export_data",
      args: {
        destination: "clipboard",
        options: {
          scope: "Selected",
          format: "urls",
          credentials: true,
          ids: [1, 2],
        },
      },
    },
  ]);
});

test("selected proxies with unknown protocols copy their original lines", async ({
  page,
}) => {
  await previewWithRows(page);
  await page.getByLabel("Select proxy3.example:8080", { exact: true }).check();
  await page
    .getByRole("button", { name: "Copy selected (1)", exact: true })
    .click();
  const calls = await page.evaluate<{ args: { options: ExportOptions } }[]>(
    "window.testCalls.filter(call => call.command === 'export_data')",
  );
  expect(calls[0].args.options).toEqual({
    scope: "Selected",
    format: "original",
    credentials: true,
    ids: [3],
  });
});

test("country detection can be disabled and the preference survives reload", async ({
  page,
}) => {
  await previewWithRows(page);
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await expect(page.getByLabel("Detect proxy country")).toBeChecked();
  await page.getByLabel("Detect proxy country").uncheck();
  await page
    .getByRole("button", { name: "Save settings", exact: true })
    .click();
  await expect(page.getByRole("dialog")).not.toBeVisible();
  await page.reload();
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await expect(page.getByLabel("Detect proxy country")).not.toBeChecked();
});

test("browser preview makes the desktop requirement clear", async ({
  page,
}) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("/");
  await expect(
    page.getByRole("heading", { name: "Proxy checker" }),
  ).toBeVisible();
  await expect(
    page.getByText("This is a browser preview.", { exact: false }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Check all", exact: true }),
  ).toBeDisabled();
  await expect(
    page.getByRole("button", { name: "Copy failed", exact: true }),
  ).toBeDisabled();
  await expect(
    page.getByRole("button", { name: "Add your first proxies" }),
  ).toBeDisabled();
  expect(errors).toEqual([]);
  await page.screenshot({
    path: "artifacts/browser-empty.png",
    fullPage: true,
  });
});

test("help is keyboard accessible and explains protocol semantics", async ({
  page,
}) => {
  await page.goto("/");
  await page.getByRole("button", { name: "Formats & help" }).click();
  const dialog = page.getByRole("dialog", { name: "Formats & help" });
  await expect(dialog).toBeVisible();
  await expect(
    dialog.getByText("means TLS to the proxy itself", { exact: false }),
  ).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(dialog).not.toBeVisible();
});

test("layout remains within the minimum desktop width", async ({ page }) => {
  await page.setViewportSize({ width: 1000, height: 650 });
  await page.goto("/");
  const widths = await page.evaluate(() => ({
    viewport: innerWidth,
    content: document.documentElement.scrollWidth,
  }));
  expect(widths.content).toBeLessThanOrEqual(widths.viewport);
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await expect(
    page.getByRole("dialog", { name: "Check settings" }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Save settings" }),
  ).toBeVisible();
});

test("backup controls explain file contents and fit the minimum window", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1000, height: 650 });
  await page.goto("/");
  await page
    .getByRole("button", { name: "Backup & restore", exact: true })
    .click();
  const dialog = page.getByRole("dialog", { name: "Backup & restore" });
  await expect(dialog).toBeVisible();
  await expect(
    dialog.getByText("without encryption", { exact: false }),
  ).toBeVisible();
  await dialog.getByLabel("Include in backup").selectOption("settings");
  await expect(dialog.getByLabel("Include in backup")).toHaveValue("settings");
  await expect(
    dialog.getByRole("button", { name: "Export backup", exact: true }),
  ).toBeDisabled();
  await expect(
    dialog.getByRole("button", { name: "Import backup", exact: true }),
  ).toBeDisabled();
  await expect(
    dialog.getByRole("button", { name: "Close", exact: true }),
  ).toBeVisible();
  const dimensions = await dialog.evaluate((el) => ({
    width: el.scrollWidth,
    visible: el.clientWidth,
    bottom: el.getBoundingClientRect().bottom,
  }));
  expect(dimensions.width).toBeLessThanOrEqual(dimensions.visible);
  expect(dimensions.bottom).toBeLessThanOrEqual(650);
  await page.keyboard.press("Escape");
  await expect(dialog).not.toBeVisible();
});
