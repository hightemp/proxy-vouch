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
            speed: {
              outcome: index === 0 ? "Completed" : "Failed",
              downloadMbps: index === 0 ? 8.39 : null,
              requestedBytes: 1_048_576,
              receivedBytes: index === 0 ? 1_048_576 : 0,
              durationMs: index === 0 ? 1000 : 100,
              limit: index === 0 ? "NotObserved" : "Signaled",
              httpStatus: index === 0 ? 200 : 429,
              proxyHttpStatus: null,
              checkUrl: "https://download.example/data",
              message:
                index === 0
                  ? "The sample passed; total traffic quota is unknown."
                  : "The download endpoint returned HTTP 429; proxy traffic quota is unknown.",
            },
            anonymity: {
              level: index === 0 ? "Elite" : "Transparent",
              message:
                index === 0
                  ? "No additional proxy indicators."
                  : "The direct IP was visible.",
              checkUrl: "http://judge.example/get",
              observedIp: `198.51.100.${index + 1}`,
              proxyHeaders: index === 0 ? [] : ["x-forwarded-for"],
            },
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

test("anonymity is visible, searchable and explained in proxy details", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1000, height: 650 });
  await previewWithRows(page);
  await expect(
    page.locator(".proxy-row").first().locator(".anonymity-tag"),
  ).toHaveText("Elite");
  await expect(
    page.locator(".proxy-row").nth(1).locator(".anonymity-tag"),
  ).toHaveText("Transparent");
  await expect(
    page.locator(".proxy-row").nth(2).locator(".anonymity-tag"),
  ).toHaveText("—");
  await page.getByLabel("Search proxies").fill("Transparent");
  await expect(page.locator(".proxy-row")).toHaveCount(1);
  await page
    .getByLabel("Details for proxy2.example:8080", { exact: true })
    .click();
  const dialog = page.getByRole("dialog", { name: "proxy2.example:8080" });
  await expect(
    dialog.getByText("HTTP anonymity check", { exact: true }),
  ).toBeVisible();
  await expect(
    dialog.getByText("The direct IP was visible.", { exact: true }),
  ).toBeVisible();
  await expect(
    dialog.getByText("Relevant headers: x-forwarded-for"),
  ).toBeVisible();
  await page.keyboard.press("Escape");
  await page.getByLabel("Search proxies").fill("");
  await page.getByLabel("Sort proxies").selectOption("anonymity");
  await expect(
    page.locator(".proxy-row").last().locator(".anonymity-tag"),
  ).toHaveText("—");
});

test("anonymity is enabled by default and disabling it is saved for the next check", async ({
  page,
}) => {
  await previewWithRows(page);
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await expect(page.getByLabel("Check proxy anonymity")).toBeChecked();
  await page.getByLabel("Check proxy anonymity").uncheck();
  await page
    .getByRole("button", { name: "Save settings", exact: true })
    .click();
  await expect(page.getByRole("dialog")).not.toBeVisible();
  await page.reload();
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await expect(page.getByLabel("Check proxy anonymity")).not.toBeChecked();
  await page.keyboard.press("Escape");
  await page.getByRole("button", { name: "Check all", exact: true }).click();
  const calls = await page.evaluate<
    { args: { settings: { anonymityCheck: boolean } } }[]
  >("window.testCalls.filter(call => call.command === 'start_check')");
  expect(calls).toHaveLength(1);
  expect(calls[0].args.settings.anonymityCheck).toBe(false);
});

for (const width of [1000, 1320]) {
  test(`speed, tested volume and limit responses fit a ${width}px window`, async ({
    page,
  }) => {
    await page.setViewportSize({ width, height: 650 });
    await previewWithRows(page);
    await expect(
      page.locator(".proxy-row").first().locator(".speed-cell"),
    ).toContainText("8.39 Mbps");
    await expect(
      page.locator(".proxy-row").first().locator(".speed-cell"),
    ).toContainText("Passed 1 MiB");
    await expect(
      page.locator(".proxy-row").nth(1).locator(".speed-cell"),
    ).toContainText("Limit response");
    await expect(
      page.locator(".proxy-row").nth(1).locator(".status-working"),
    ).toBeVisible();
    await expect(
      page.locator(".proxy-row").nth(2).locator(".speed-cell"),
    ).toHaveText("—");
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
    await page.getByLabel("Sort proxies").selectOption("speed");
    await expect(
      page.locator(".proxy-row").first().locator(".speed-cell"),
    ).toContainText("8.39 Mbps");
    await page.screenshot({
      path: `artifacts/speed-volume-${width}.png`,
      fullPage: true,
    });
    await page.getByLabel("Search proxies").fill("limit");
    await expect(page.locator(".proxy-row")).toHaveCount(1);
    await page
      .getByLabel("Details for proxy2.example:8080", { exact: true })
      .click();
    const dialog = page.getByRole("dialog", { name: "proxy2.example:8080" });
    await expect(
      dialog.getByText("Download speed and transfer test", { exact: true }),
    ).toBeVisible();
    await expect(
      dialog.getByText(
        "The download endpoint returned HTTP 429; proxy traffic quota is unknown.",
      ),
    ).toBeVisible();
    await expect(
      dialog.getByText("Total / remaining traffic quota", { exact: true }),
    ).toBeVisible();
    await expect(
      dialog.getByText("Unknown — requires provider information", {
        exact: true,
      }),
    ).toBeVisible();
  });
}

test("speed testing defaults on and saves the selected sample size and disabled state", async ({
  page,
}) => {
  await previewWithRows(page);
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await expect(
    page.getByLabel("Check download speed and transfer limits"),
  ).toBeChecked();
  await expect(page.getByLabel("Speed test size (MiB)")).toHaveValue("1");
  await page.getByLabel("Speed test size (MiB)").fill("4");
  await page.getByLabel("Check download speed and transfer limits").uncheck();
  await expect(page.getByLabel("Speed test size (MiB)")).toBeDisabled();
  await page
    .getByRole("button", { name: "Save settings", exact: true })
    .click();
  await expect(page.getByRole("dialog")).not.toBeVisible();
  await page.reload();
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await expect(
    page.getByLabel("Check download speed and transfer limits"),
  ).not.toBeChecked();
  await expect(page.getByLabel("Speed test size (MiB)")).toHaveValue("4");
  await page.keyboard.press("Escape");
  await page.getByRole("button", { name: "Check all", exact: true }).click();
  const calls = await page.evaluate<
    { args: { settings: { speedCheck: boolean; speedTestMib: number } } }[]
  >("window.testCalls.filter(call => call.command === 'start_check')");
  expect(calls).toHaveLength(1);
  expect(calls[0].args.settings).toMatchObject({
    speedCheck: false,
    speedTestMib: 4,
  });
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
