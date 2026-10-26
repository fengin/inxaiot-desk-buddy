export interface ScreenImageRect { left: number; top: number; width: number; height: number }
export interface ScreenTapPoint { x: number; y: number }

/** 图片采用 object-fit: contain；留白、无效尺寸和边界外点击不发送。 */
export function screenTapPoint(clientX: number, clientY: number, rect: ScreenImageRect,
  imageWidth: number, imageHeight: number): ScreenTapPoint | null {
  if (![clientX, clientY, rect.left, rect.top, rect.width, rect.height, imageWidth, imageHeight].every(Number.isFinite)
    || rect.width <= 0 || rect.height <= 0 || imageWidth <= 0 || imageHeight <= 0) return null;
  const scale = Math.min(rect.width / imageWidth, rect.height / imageHeight);
  const left = rect.left + (rect.width - imageWidth * scale) / 2;
  const top = rect.top + (rect.height - imageHeight * scale) / 2;
  const x = (clientX - left) / scale;
  const y = (clientY - top) / scale;
  if (x < 0 || y < 0 || x >= imageWidth || y >= imageHeight) return null;
  return { x: Math.floor(x), y: Math.floor(y) };
}

export const SCREEN_REMOTE_TEXT_LIMIT = 128;

/** 不修剪账号/密码空格，也不回显输入内容；不支持的字符必须明确拒绝。 */
export function screenRemoteTextError(text: string, maxLength = SCREEN_REMOTE_TEXT_LIMIT): string {
  if (!text.length) return "请输入内容";
  if (text.length > maxLength) return `一次最多输入${maxLength}个字符`;
  if (!/^[\x20-\x7e]+$/.test(text)) return "仅支持英文、数字和常用符号，暂不支持中文、Emoji或换行";
  return "";
}
