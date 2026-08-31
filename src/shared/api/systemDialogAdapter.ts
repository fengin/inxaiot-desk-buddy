export interface FileDialogFilter {
  name: string;
  extensions: string[];
}

export interface SystemDialogAdapter {
  readonly real: boolean;
  selectDirectory(title: string): Promise<string | null>;
  selectFile(title: string, filters: FileDialogFilter[]): Promise<string | null>;
  saveFile(title: string, filters: FileDialogFilter[], defaultPath?: string): Promise<string | null>;
}

let adapter: SystemDialogAdapter | undefined;

export function configureSystemDialogAdapter(next: SystemDialogAdapter) {
  adapter = next;
}

export function useSystemDialogAdapter() {
  if (!adapter) throw new Error("系统对话框Adapter尚未初始化");
  return adapter;
}
