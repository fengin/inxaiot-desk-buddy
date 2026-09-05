import type { SystemDialogAdapter } from "@/shared/api/systemDialogAdapter";

export class FixtureSystemDialogAdapter implements SystemDialogAdapter {
  readonly real = false;

  async selectDirectory() {
    return "D:\\INX\\DeskBuddy-Fixture";
  }

  async selectFile() {
    return null;
  }
}
