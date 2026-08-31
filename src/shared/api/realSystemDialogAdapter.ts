import { open, save } from "@tauri-apps/plugin-dialog";

import type { FileDialogFilter, SystemDialogAdapter } from "@/shared/api/systemDialogAdapter";

export class RealSystemDialogAdapter implements SystemDialogAdapter {
  readonly real = true;

  async selectDirectory(title: string) {
    const selected = await open({ directory: true, multiple: false, title });
    return typeof selected === "string" ? selected : null;
  }

  async selectFile(title: string, filters: FileDialogFilter[]) {
    const selected = await open({ directory: false, multiple: false, title, filters });
    return typeof selected === "string" ? selected : null;
  }

  async saveFile(title: string, filters: FileDialogFilter[], defaultPath?: string) {
    const selected = await save({ title, filters, defaultPath });
    return typeof selected === "string" ? selected : null;
  }
}
