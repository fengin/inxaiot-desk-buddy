import type { CommandErrorDto } from "@/shared/model/activity";

export function commandErrorText(error: unknown, fallback: string) {
  const dto = error as Partial<CommandErrorDto> | undefined;
  return dto?.params?.summary ?? (error instanceof Error ? error.message : fallback);
}

export function commandErrorCode(error: unknown) {
  return (error as Partial<CommandErrorDto> | undefined)?.code;
}
