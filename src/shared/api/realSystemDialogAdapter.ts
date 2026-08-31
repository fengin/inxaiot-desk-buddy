import { open } from "@tauri-apps/plugin-dialog";

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
}
