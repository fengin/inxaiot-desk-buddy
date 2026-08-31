import { createI18n } from "vue-i18n";

export const i18n = createI18n({
  legacy: false,
  locale: "zh-CN",
  fallbackLocale: "zh-CN",
  messages: {
    "zh-CN": {
      app: {
        name: "INX 实施工作台",
        tagline: "项目实施与边缘资源维护"
      }
    }
  }
});

