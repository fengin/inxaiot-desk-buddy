import { defineComponent, h, ref, shallowRef } from "vue";
import { flushPromises, mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";
import { NCheckbox, NSelect } from "naive-ui";
import ScreenInstallFields from "./ScreenInstallFields.vue";
import type { ScreenApkSelection } from "@/shared/model/screen";

function render() {
  const file = shallowRef<ScreenApkSelection | File | null>(null), reinstall = ref(false), disabled = ref(false), sizeWarning = ref("");
  const wrapper = mount(defineComponent({ setup: () => () => h(ScreenInstallFields, {
    file: file.value, reinstall: reinstall.value, disabled: disabled.value, sizeWarning: sizeWarning.value,
    'onUpdate:file': (value: ScreenApkSelection | File | null) => { file.value = value; },
    'onUpdate:reinstall': (value: boolean) => { reinstall.value = value; }
  }) }));
  async function choose(next?: File) {
    const input = wrapper.get<HTMLInputElement>('[data-testid="screen-apk-input"]');
    Object.defineProperty(input.element, 'files', { configurable: true, value: next ? [next] : [] });
    await input.trigger('change'); await flushPromises();
  }
  return { wrapper, file, disabled, sizeWarning, choose };
}

describe("本地 APK 文件选择", () => {
  it("真实 File 仅保留当前选择，展示文件名大小，不猜测版本或架构", async () => {
    const { wrapper, file, choose } = render();
    try {
      const apk = new File(['sample'], 'xiaoxin-9.9.9-arm64.apk', { lastModified: 123 });
      await choose(apk);
      expect(file.value).toBe(apk);
      expect(wrapper.get<HTMLInputElement>('[data-testid="screen-apk-selected"] input').element.value).toBe(apk.name);
      expect(wrapper.get('[data-testid="screen-apk-selected"] .n-input').attributes('title')).toBe(apk.name);
      expect(wrapper.text()).not.toContain('目标版本');
      expect(wrapper.text()).not.toContain('安装包架构');
      expect(wrapper.text()).not.toContain('演示安装包');
      expect(wrapper.get<HTMLInputElement>('[data-testid="screen-apk-input"]').element.value).toBe('');
    } finally { wrapper.unmount(); }
  });

  it("取消选择保留旧文件，更换文件后使用新 File", async () => {
    const { wrapper, file, choose } = render();
    try {
      const first = new File(['first'], 'first.apk');
      const second = new File(['second'], 'second.APK');
      await choose(first); await choose();
      expect(file.value).toBe(first);
      await choose(second);
      expect(file.value).toBe(second);
      expect(wrapper.get<HTMLInputElement>('[data-testid="screen-apk-selected"] input').element.value).toBe('second.APK');
    } finally { wrapper.unmount(); }
  });

  it("不接受非 APK 或空文件，明确提示并清除无效选择", async () => {
    const { wrapper, file, choose } = render();
    try {
      await choose(new File(['valid'], 'valid.apk'));
      await choose(new File(['invalid'], 'other.zip'));
      expect(file.value).toBeNull();
      expect(wrapper.get('[role="alert"]').text()).toContain('.apk');
      await choose(new File([], 'empty.apk'));
      expect(file.value).toBeNull();
      expect(wrapper.get('[role="alert"]').text()).toContain('空');
    } finally { wrapper.unmount(); }
  });

  it("检查期间禁用选择，迟到的文件 change 不覆盖原文件", async () => {
    const { wrapper, file, disabled, choose } = render();
    try {
      const first = new File(['first'], 'first.apk');
      await choose(first); disabled.value = true; await flushPromises();
      await choose(new File(['late'], 'late.apk'));
      expect(file.value).toBe(first);
      expect(wrapper.get('[data-testid="screen-apk-picker"]').attributes('disabled')).toBeDefined();
    } finally { wrapper.unmount(); }
  });

  it("文件窗口返回前组件卸载，迟到 change 不写入旧选择", async () => {
    const { wrapper, file, choose } = render();
    const first = new File(['first'], 'first.apk');
    await choose(first);
    const input = wrapper.get<HTMLInputElement>('[data-testid="screen-apk-input"]').element;
    wrapper.unmount();
    Object.defineProperty(input, 'files', { configurable: true, value: [new File(['late'], 'late.apk')] });
    input.dispatchEvent(new Event('change')); await flushPromises();
    expect(file.value).toBe(first);
  });

  it("尺寸警告只禁用文件相关控件，并拦截迟到选择", async () => {
    const { wrapper, file, sizeWarning, choose } = render();
    try {
      const first = new File(['first'], 'first.apk'); await choose(first);
      sizeWarning.value = '一批只能操作同规格尺寸屏'; await flushPromises();
      expect(wrapper.get('[data-testid="screen-apk-size-warning"]').text()).toBe(sizeWarning.value);
      expect(wrapper.get('[data-testid="screen-apk-picker"]').attributes('disabled')).toBeDefined();
      expect(wrapper.get<HTMLInputElement>('[data-testid="screen-apk-input"]').element.disabled).toBe(true);
      expect(wrapper.get<HTMLInputElement>('[data-testid="screen-apk-selected"] input').element.disabled).toBe(true);
      expect(wrapper.getComponent(NSelect).props('disabled')).toBe(false);
      expect(wrapper.getComponent(NCheckbox).props('disabled')).toBe(false);
      await choose(new File(['late'], 'late.apk'));
      expect(file.value).toBe(first);
      sizeWarning.value = ''; await flushPromises();
      expect(wrapper.get('[data-testid="screen-apk-picker"]').attributes('disabled')).toBeUndefined();
    } finally { wrapper.unmount(); }
  });
});
