import { test as base, expect, type Locator, type Page } from "@playwright/test";

// The fake backend's `declare global` types `window.__e2e` in the evaluate callbacks.
import type { Command, CommandArgs } from "./fake-backend/handlers";
import type {} from "./fake-backend/index";
import type { Seed, State } from "./fake-backend/state";

const NO_HANDLER = "fake backend: no handler for ";

/** Drives the fake backend of the page under test (see `fake-backend/index.ts`). */
export class FakeBackend {
  private readonly page: Page;

  constructor(page: Page) {
    this.page = page;
  }

  private async ready() {
    await this.page.waitForFunction(() => window.__e2e !== undefined);
  }

  /** The data the backend starts with. Call before the first `page.goto`. */
  async seed(seed: Seed) {
    await this.page.addInitScript((s) => {
      window.__E2E_SEED__ = s;
    }, seed);
  }

  /** The args of every call to `cmd` since the page last loaded. */
  async calls<C extends Command>(cmd: C): Promise<CommandArgs<C>[]> {
    await this.ready();

    const args = await this.page.evaluate(
      (c) => window.__e2e!.calls.filter((call) => call.cmd === c).map((call) => call.args),
      cmd,
    );

    // SAFETY: the fake records each invoke's args as the app sent them, in the
    // shape the command's handler declares.
    return args as CommandArgs<C>[];
  }

  async failNext(cmd: string, message = "fake failure") {
    await this.ready();

    return this.page.evaluate(([c, m]) => window.__e2e!.failNext(c, m), [cmd, message] as const);
  }

  async hold(cmd: string) {
    await this.ready();

    return this.page.evaluate((c) => window.__e2e!.hold(c), cmd);
  }

  async release(cmd: string) {
    await this.ready();

    return this.page.evaluate((c) => window.__e2e!.release(c), cmd);
  }

  async holdResponse(cmd: string) {
    await this.ready();

    return this.page.evaluate((c) => window.__e2e!.holdResponse(c), cmd);
  }

  async releaseResponse(cmd: string) {
    await this.ready();

    return this.page.evaluate((c) => window.__e2e!.releaseResponse(c), cmd);
  }

  async emit<T>(event: string, payload: T) {
    await this.ready();

    return this.page.evaluate(([e, p]) => window.__e2e!.emit(e, p), [event, payload] as const);
  }

  async patch(partial: Partial<State>) {
    await this.ready();

    return this.page.evaluate((p) => window.__e2e!.patch(p), partial);
  }
}

export const test = base.extend<{ backend: FakeBackend }>({
  // `provide` is Playwright's `use`, renamed so the React hooks lint leaves it alone.
  backend: async ({ page }, provide) => {
    // The app must not reach the network (e.g. GitHub avatars in the market).
    await page.route(/^https?:\/\/(?!127\.0\.0\.1[:/])/, (route) => route.abort());

    const missing = new Set<string>();
    page.on("console", (message) => {
      const text = message.text();

      if (text.startsWith(NO_HANDLER)) missing.add(text.slice(NO_HANDLER.length));
    });

    await provide(new FakeBackend(page));

    expect([...missing], "commands the fake backend has no handler for").toEqual([]);
  },
});

export { expect };

/**
 * Drag a sortable `item` by its grip `handle` onto `target` with the mouse, in
 * small steps so dnd-kit's pointer sensor activates and tracks the move.
 */
export async function dragOnto(page: Page, item: Locator, handle: Locator, target: Locator) {
  await item.hover(); // grips only show on hover

  const [itemBox, handleBox, targetBox] = await Promise.all([
    item.boundingBox(),
    handle.boundingBox(),
    target.boundingBox(),
  ]);

  if (!itemBox || !handleBox || !targetBox) throw new Error("dragOnto: element not visible");

  const center = (box: { x: number; y: number; width: number; height: number }) => ({
    x: box.x + box.width / 2,
    y: box.y + box.height / 2,
  });

  const start = center(handleBox);
  // Moves the item's centre onto the target's centre.
  const dx = center(targetBox).x - center(itemBox).x;
  const dy = center(targetBox).y - center(itemBox).y;
  await page.mouse.move(start.x, start.y);
  await page.mouse.down();
  await page.mouse.move(start.x + dx, start.y + dy, { steps: 20 });
  await page.mouse.up();
}
